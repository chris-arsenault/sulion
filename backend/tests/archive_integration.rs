#![cfg(feature = "integration-tests")]

//! Transcript archive integration: a session goes through the real ingest
//! path, is exported to a directory store, purged to its turn digest, and
//! restored by replay. Every consumer that must keep working on archived
//! history is checked before and after: cost totals, file history, search
//! sources, the turn digest, the admin reindex, and the late-append guard.

use std::io::Write;
use std::path::PathBuf;

use sulion::archive::{self, ArchiveConfig, ObjectStore, RestoreScope};
use sulion::db;
use sulion::ingest::{rebuild_ingest_derivatives, Ingester, IngesterConfig};
use uuid::Uuid;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE retrieval_embedding_backfills, retrieval_embedding_sources, retrieval_embeddings, \
         plan_events, plan_attachments, plan_phases, plans, session_activity_state, \
         events, ingester_state, ingest_jobs, claude_sessions, pty_sessions, repos, \
         repo_runtime_state, repo_dirty_paths, timeline_session_state, \
         archive_requests, usage_daily_rollup, usage_rollup_contributions, \
         file_activity_daily, file_activity_contributions, \
         workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate archive test tables");
    sqlx::query(
        "UPDATE archive_state SET last_cycle_started_at = NULL, last_cycle_completed_at = NULL, \
                last_dump_key = NULL, last_dump_at = NULL, purge_enabled = FALSE, \
                purge_enabled_at = NULL, store = NULL, loop_started_at = NULL WHERE id = 1",
    )
    .execute(&pool)
    .await
    .expect("reset archive state");
    pool
}

/// A Claude transcript in a temp project root, correlated to a dead PTY in
/// repo `demo` so tool calls on `/home/sulion/repos/demo/...` project as
/// file touches.
struct Fixture {
    root: tempfile::TempDir,
    store_dir: tempfile::TempDir,
    session_uuid: Uuid,
    pty_id: Uuid,
}

impl Fixture {
    async fn new(pool: &db::Pool) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let store_dir = tempfile::tempdir().expect("store tempdir");
        let session_uuid = Uuid::new_v4();
        let pty_id = Uuid::new_v4();
        std::fs::create_dir_all(root.path().join("demo-hash")).unwrap();
        sqlx::query("INSERT INTO repos (name, path) VALUES ('demo', '/home/sulion/repos/demo')")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at, \
                                       current_session_uuid, current_session_agent) \
             VALUES ($1, 'demo', '/home/sulion/repos/demo', 'dead', NOW(), $2, 'claude-code')",
        )
        .bind(pty_id)
        .bind(session_uuid)
        .execute(pool)
        .await
        .unwrap();
        Self {
            root,
            store_dir,
            session_uuid,
            pty_id,
        }
    }

    fn jsonl_path(&self) -> PathBuf {
        self.root
            .path()
            .join("demo-hash")
            .join(format!("{}.jsonl", self.session_uuid))
    }

    fn append(&self, line: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.jsonl_path())
            .expect("open for append");
        f.write_all(line.as_bytes()).expect("write");
        f.write_all(b"\n").expect("newline");
        f.flush().ok();
    }

    fn ingester_config(&self) -> IngesterConfig {
        IngesterConfig::new(self.root.path().to_path_buf())
    }

    fn store(&self) -> ObjectStore {
        ObjectStore::Dir {
            root: self.store_dir.path().to_path_buf(),
        }
    }

    /// The shipped configuration: purging off, as `compose.yaml` starts.
    fn archive_config(&self, purge_after_days: i64) -> ArchiveConfig {
        ArchiveConfig {
            store: self.store(),
            db_url: test_db_url().unwrap(),
            min_idle_days: 1,
            purge_after_days,
            interval_days: 30,
            purge_enabled: false,
            dump_enabled: false,
        }
    }

    /// The configuration after the commit that sets
    /// `SULION_ARCHIVE_PURGE_ENABLED` to `1`.
    fn purging_config(&self, purge_after_days: i64) -> ArchiveConfig {
        ArchiveConfig {
            purge_enabled: true,
            ..self.archive_config(purge_after_days)
        }
    }

    /// One prompt, one assistant turn that edits a repo file, with usage.
    /// Timestamps are far enough back to be idle for any window.
    fn write_transcript(&self) {
        self.append(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-01-10T10:00:00Z","cwd":"/home/sulion/repos/demo","message":{"role":"user","content":"please rename the widget helper"}}"#,
        );
        self.append(
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-10T10:00:05Z","cwd":"/home/sulion/repos/demo","message":{"id":"msg_1","model":"claude-opus-5","role":"assistant","content":[{"type":"text","text":"Renaming the helper in src/widget.rs now."},{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"/home/sulion/repos/demo/src/widget.rs","old_string":"fn helper","new_string":"fn widget_helper"}}],"usage":{"input_tokens":120,"cache_creation_input_tokens":400,"cache_read_input_tokens":900,"output_tokens":60}}}"#,
        );
        self.append(
            r#"{"type":"user","uuid":"u2","parentUuid":"a1","timestamp":"2026-01-10T10:00:06Z","cwd":"/home/sulion/repos/demo","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"The file has been updated."}]}}"#,
        );
        self.append(
            r#"{"type":"assistant","uuid":"a2","parentUuid":"u2","timestamp":"2026-01-10T10:00:09Z","cwd":"/home/sulion/repos/demo","message":{"id":"msg_2","model":"claude-opus-5","role":"assistant","content":[{"type":"text","text":"Done: the helper is now widget_helper and the callers compile."}],"usage":{"input_tokens":40,"cache_creation_input_tokens":0,"cache_read_input_tokens":1300,"output_tokens":30}}}"#,
        );
    }
}

async fn count(pool: &db::Pool, table: &str, session: Uuid) -> i64 {
    let (n,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*)::BIGINT FROM {table} WHERE session_uuid = $1"
    ))
    .bind(session)
    .fetch_one(pool)
    .await
    .unwrap();
    n
}

async fn all_time_tokens(pool: &db::Pool) -> (i64, i64, i64) {
    let metrics = sulion::metrics::portfolio_metrics(pool).await.unwrap();
    (
        metrics.usage.all_time.input_tokens,
        metrics.usage.all_time.cached_input_tokens,
        metrics.usage.all_time.output_tokens,
    )
}

async fn repo_usage(pool: &db::Pool, repo: &str) -> Option<(i64, i64)> {
    let metrics = sulion::metrics::portfolio_metrics(pool).await.unwrap();
    metrics
        .usage
        .per_repo
        .iter()
        .find(|row| row.repo == repo)
        .map(|row| (row.all_time.input_tokens, row.all_time.output_tokens))
}

async fn ingest(pool: &db::Pool, fx: &Fixture) {
    let ingester = Ingester::new();
    ingester.tick(pool, &fx.ingester_config()).await.unwrap();
    ingester.tick(pool, &fx.ingester_config()).await.unwrap();
}

#[tokio::test]
async fn export_verifies_object_and_records_archive_columns() {
    let pool = fresh_pool().await;
    let fx = Fixture::new(&pool).await;
    fx.write_transcript();
    ingest(&pool, &fx).await;
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 4);

    let dry = archive::run_cycle(&pool, &fx.archive_config(90), true)
        .await
        .unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.eligible_for_export, 1);
    assert_eq!(dry.eligible_for_purge, 0);
    assert_eq!(
        count(&pool, "events", fx.session_uuid).await,
        4,
        "dry run touches nothing"
    );

    let outcome = archive::run_cycle(&pool, &fx.archive_config(90), false)
        .await
        .unwrap();
    assert_eq!(outcome.exported, 1);
    assert_eq!(outcome.export_failures, 0);
    assert_eq!(outcome.purged, 0, "a 90-day grace purges nothing today");
    assert!(
        !outcome.purge_enabled,
        "the shipped configuration never deletes"
    );

    let (key, sha, bytes, events, purged): (
        String,
        String,
        i64,
        i64,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT archive_key, archive_sha256, archive_bytes, archive_events, purged_at \
               FROM claude_sessions WHERE session_uuid = $1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(key.starts_with("sessions/claude-code/2026/01/"), "{key}");
    assert!(key.ends_with(".jsonl.zst"));
    assert_eq!(sha.len(), 64);
    assert!(bytes > 0);
    assert_eq!(events, 4);
    assert!(purged.is_none());

    let head = fx.store().head(&key).await.unwrap().expect("object exists");
    assert_eq!(
        head.metadata.get("sha256").map(String::as_str),
        Some(sha.as_str())
    );
    assert_eq!(head.metadata.get("events").map(String::as_str), Some("4"));

    // The object round-trips: same lines, original offsets, verified hash.
    let lines = archive::export::read_session_lines(&fx.store(), &key, &sha)
        .await
        .unwrap();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0].o, 0);
    assert!(lines[1].o > 0);
    assert_eq!(lines[0].k, "user");

    // Unchanged since export: not eligible again.
    let again = archive::run_cycle(&pool, &fx.archive_config(90), true)
        .await
        .unwrap();
    assert_eq!(again.eligible_for_export, 0);

    // A live PTY's current session is never eligible.
    sqlx::query("UPDATE pty_sessions SET state = 'live' WHERE id = $1")
        .bind(fx.pty_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE claude_sessions SET archived_at = NULL WHERE session_uuid = $1")
        .bind(fx.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    let live = archive::run_cycle(&pool, &fx.archive_config(90), true)
        .await
        .unwrap();
    assert_eq!(live.eligible_for_export, 0);
}

#[tokio::test]
async fn purge_keeps_the_digest_rollups_and_every_consumer_working() {
    let pool = fresh_pool().await;
    let fx = Fixture::new(&pool).await;
    fx.write_transcript();
    ingest(&pool, &fx).await;

    let touches_before = count(&pool, "timeline_file_touches", fx.session_uuid).await;
    assert!(
        touches_before >= 1,
        "the Edit call must project as a file touch"
    );
    let turns_before = count(&pool, "timeline_turns", fx.session_uuid).await;
    assert!(turns_before >= 1);
    let tokens_before = all_time_tokens(&pool).await;
    assert!(tokens_before.0 > 0 && tokens_before.2 > 0);
    let repo_before = repo_usage(&pool, "demo").await.expect("repo attributed");
    let trace_before = sulion::ingest::load_repo_file_trace(&pool, "demo", "src/widget.rs")
        .await
        .unwrap();
    assert!(!trace_before.is_empty());
    // A live turn composes its digest on read; the purge stores that text.
    let first_turn: i64 = sqlx::query_scalar(
        "SELECT turn_id FROM timeline_turns WHERE session_uuid = $1 ORDER BY turn_id LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let markdown_before = sulion::ingest::load_timeline_turn_detail(
        &pool,
        fx.session_uuid,
        first_turn,
        &Default::default(),
    )
    .await
    .unwrap()
    .expect("first turn")
    .markdown;
    assert!(markdown_before.contains("widget_helper"));

    // With purging off a zero-grace cycle still deletes nothing.
    let closed = archive::run_cycle(&pool, &fx.archive_config(0), false)
        .await
        .unwrap();
    assert_eq!(closed.exported, 1);
    assert_eq!(closed.purged, 0);
    assert!(!closed.purge_enabled);
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 4);

    // The first backup is verified; the loop that starts with purging off
    // records that, and the loop after the enabling commit records when.
    let verified = archive::export::verify_archives(&pool, &fx.store(), true, None)
        .await
        .unwrap();
    assert_eq!(verified.sessions_archived, 1);
    assert_eq!(verified.ok, 1, "{:?}", verified.problems);
    assert_eq!(verified.missing + verified.mismatched, 0);
    let before_loop = archive::status(&pool).await.unwrap();
    assert!(!before_loop.configured, "no loop has recorded a store yet");
    archive::record_store(&pool, &fx.archive_config(0))
        .await
        .unwrap();
    let off = archive::status(&pool).await.unwrap();
    assert!(off.configured);
    assert!(!off.purge_enabled);
    assert!(off.purge_enabled_at.is_none());
    archive::record_store(&pool, &fx.purging_config(0))
        .await
        .unwrap();
    let status = archive::status(&pool).await.unwrap();
    assert!(status.purge_enabled);
    let enabled_at = status
        .purge_enabled_at
        .expect("recorded when purging came on");
    assert_eq!(
        status.store.as_deref(),
        Some(fx.store_dir.path().to_str().unwrap())
    );
    archive::record_store(&pool, &fx.purging_config(0))
        .await
        .unwrap();
    let restarted = archive::status(&pool).await.unwrap();
    assert_eq!(
        restarted.purge_enabled_at,
        Some(enabled_at),
        "a restart keeps the first enable time"
    );

    // Nothing new to export; the purge now runs with no grace.
    let outcome = archive::run_cycle(&pool, &fx.purging_config(0), false)
        .await
        .unwrap();
    assert_eq!(outcome.exported, 0);
    assert_eq!(outcome.purged, 1, "{outcome:?}");
    assert_eq!(outcome.purge_failures, 0);
    assert_eq!(outcome.events_deleted, 4);

    // Gone: events, blocks, operations, touches, per-session usage.
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 0);
    assert_eq!(count(&pool, "event_blocks", fx.session_uuid).await, 0);
    assert_eq!(
        count(&pool, "timeline_operations", fx.session_uuid).await,
        0
    );
    assert_eq!(
        count(&pool, "timeline_file_touches", fx.session_uuid).await,
        0
    );
    assert_eq!(
        count(&pool, "agent_model_usage_daily", fx.session_uuid).await,
        0
    );
    assert_eq!(count(&pool, "agent_usage_daily", fx.session_uuid).await, 0);
    assert_eq!(
        count(&pool, "agent_session_usage", fx.session_uuid).await,
        0
    );

    // Kept: the digest with its markdown and file list.
    assert_eq!(
        count(&pool, "timeline_turns", fx.session_uuid).await,
        turns_before
    );
    let (markdown_after, files_json): (String, serde_json::Value) = sqlx::query_as(
        "SELECT markdown, files_json FROM timeline_turns \
          WHERE session_uuid = $1 ORDER BY turn_id LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(markdown_after, markdown_before);
    assert_eq!(count(&pool, "timeline_items", fx.session_uuid).await, 0);
    let files = files_json.as_array().expect("files_json array");
    assert!(
        files
            .iter()
            .any(|f| f["path"] == "src/widget.rs" && f["write"] == true),
        "{files_json}"
    );
    let purged_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT purged_at FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(purged_at.is_some());

    // Cost report: identical totals, still attributed to the repo.
    assert_eq!(all_time_tokens(&pool).await, tokens_before);
    assert_eq!(repo_usage(&pool, "demo").await, Some(repo_before));
    let rollup: (i64, i64) = sqlx::query_as(
        "SELECT SUM(input_tokens)::BIGINT, SUM(output_tokens)::BIGINT \
           FROM usage_daily_rollup WHERE repo = 'demo' AND model = 'claude-opus-5'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rollup, (160, 90));

    // File churn: the daily rollup and file-history both still see the edit.
    let activity: (i64, i64) = sqlx::query_as(
        "SELECT write_turns, read_turns FROM file_activity_daily \
          WHERE repo = 'demo' AND path = 'src/widget.rs' AND day = DATE '2026-01-10'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(activity.0, 1);
    let trace_after = sulion::ingest::load_repo_file_trace(&pool, "demo", "src/widget.rs")
        .await
        .unwrap();
    assert_eq!(trace_after.len(), trace_before.len());
    assert_eq!(trace_after[0].turn_id, trace_before[0].turn_id);
    assert!(trace_after[0].is_write);
    assert!(
        trace_after[0].pair_id.is_none(),
        "no operation to link on a digest"
    );

    // Search: the digest is queued for embedding, block sources are gone.
    let sources: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_family, index_status FROM retrieval_embedding_sources \
          WHERE session_uuid = $1",
    )
    .bind(fx.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!sources.is_empty());
    assert!(sources
        .iter()
        .all(|(family, status)| family == "turn_digest" && status == "pending"));

    // The admin reindex rebuilds live sessions and leaves the digest alone.
    rebuild_ingest_derivatives(&pool).await.unwrap();
    assert_eq!(
        count(&pool, "timeline_turns", fx.session_uuid).await,
        turns_before
    );
    let still: String = sqlx::query_scalar(
        "SELECT markdown FROM timeline_turns WHERE session_uuid = $1 ORDER BY turn_id LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still, markdown_before);

    // The timeline API reports the session as archived.
    let meta = sulion::ingest::load_timeline_session_meta(&pool, fx.session_uuid)
        .await
        .unwrap();
    assert!(meta.archived_at.is_some());

    // Late append: the transcript grows after the purge. The ingester asks
    // for a restore and inserts nothing.
    fx.append(
        r#"{"type":"user","uuid":"u3","parentUuid":"a2","timestamp":"2026-01-10T11:00:00Z","cwd":"/home/sulion/repos/demo","message":{"role":"user","content":"and one more thing"}}"#,
    );
    ingest(&pool, &fx).await;
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 0);
    let pending: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT kind, scope FROM archive_requests WHERE status = 'pending'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "exactly one restore is queued, not one per tick"
    );
    assert_eq!(pending[0].0, "restore");
    assert_eq!(pending[0].1["session_uuid"], fx.session_uuid.to_string());
}

#[tokio::test]
async fn restore_replays_the_archive_and_purge_after_returns_to_the_digest() {
    let pool = fresh_pool().await;
    let fx = Fixture::new(&pool).await;
    fx.write_transcript();
    ingest(&pool, &fx).await;

    let tokens_before = all_time_tokens(&pool).await;
    let ops_before = count(&pool, "timeline_operations", fx.session_uuid).await;
    let touches_before = count(&pool, "timeline_file_touches", fx.session_uuid).await;
    let offsets_before: Vec<(i64,)> = sqlx::query_as(
        "SELECT byte_offset FROM events WHERE session_uuid = $1 ORDER BY byte_offset",
    )
    .bind(fx.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();

    let outcome = archive::run_cycle(&pool, &fx.purging_config(0), false)
        .await
        .unwrap();
    assert_eq!(outcome.purged, 1);
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 0);

    // Restore through the request path the CLI and UI use.
    let scope = RestoreScope {
        session_uuid: Some(fx.session_uuid),
        ..RestoreScope::default()
    };
    let restored = archive::run_restore(&pool, &fx.archive_config(0), &scope, None)
        .await
        .unwrap();
    assert_eq!(restored.restored, 1, "{:?}", restored.errors);
    assert_eq!(restored.failures, 0);
    let one = &restored.outcomes[0];
    assert_eq!(one.lines, 4);
    assert_eq!(one.events_inserted, 4);
    assert_eq!(
        one.tokens_before, one.tokens_after,
        "replayed usage matches the rollup"
    );
    assert!(!one.purged_again);

    // Exactly as before: same offsets, operations, touches, cost, no rollup left.
    let offsets_after: Vec<(i64,)> = sqlx::query_as(
        "SELECT byte_offset FROM events WHERE session_uuid = $1 ORDER BY byte_offset",
    )
    .bind(fx.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(offsets_after, offsets_before);
    assert_eq!(
        count(&pool, "timeline_operations", fx.session_uuid).await,
        ops_before
    );
    assert_eq!(
        count(&pool, "timeline_file_touches", fx.session_uuid).await,
        touches_before
    );
    assert_eq!(all_time_tokens(&pool).await, tokens_before);
    let rollup_left: (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(input_tokens), 0)::BIGINT FROM usage_daily_rollup")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rollup_left.0, 0);
    let contributions = count(&pool, "usage_rollup_contributions", fx.session_uuid).await;
    assert_eq!(contributions, 0);
    let purged_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT purged_at FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(purged_at.is_none());
    let files_json: serde_json::Value = sqlx::query_scalar(
        "SELECT files_json FROM timeline_turns WHERE session_uuid = $1 ORDER BY turn_id LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        files_json,
        serde_json::json!([]),
        "live turns carry no digest file list"
    );

    // A whole-history replay: `--all` purges again after each session.
    let outcome = archive::run_cycle(&pool, &fx.purging_config(0), false)
        .await
        .unwrap();
    assert_eq!(
        outcome.exported, 0,
        "the archive already matches the session"
    );
    assert_eq!(outcome.purged, 1);
    let all = archive::run_restore(
        &pool,
        &fx.purging_config(0),
        &RestoreScope {
            all: true,
            ..RestoreScope::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(all.restored, 1, "{:?}", all.errors);
    assert_eq!(all.purged_again, 1);
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 0);
    assert_eq!(all_time_tokens(&pool).await, tokens_before);

    // The late-append guard then lets an appended line through once restored.
    fx.append(
        r#"{"type":"user","uuid":"u3","parentUuid":"a2","timestamp":"2026-01-10T11:00:00Z","cwd":"/home/sulion/repos/demo","message":{"role":"user","content":"and one more thing"}}"#,
    );
    ingest(&pool, &fx).await;
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 0);
    let queued = archive::requests::next_pending(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.kind, "restore");
    let scope: RestoreScope = serde_json::from_value(queued.scope).unwrap();
    archive::run_restore(&pool, &fx.archive_config(0), &scope, Some(queued.id))
        .await
        .unwrap();
    ingest(&pool, &fx).await;
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 5);
}

#[tokio::test]
async fn verify_reports_a_tampered_or_missing_object() {
    let pool = fresh_pool().await;
    let fx = Fixture::new(&pool).await;
    fx.write_transcript();
    ingest(&pool, &fx).await;
    archive::run_cycle(&pool, &fx.archive_config(90), false)
        .await
        .unwrap();
    let key: String =
        sqlx::query_scalar("SELECT archive_key FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    let shallow = archive::export::verify_archives(&pool, &fx.store(), false, None)
        .await
        .unwrap();
    assert_eq!((shallow.ok, shallow.missing, shallow.mismatched), (1, 0, 0));

    // Metadata still matches, but the bytes do not: only a deep check sees it.
    let object = fx.store_dir.path().join(&key);
    let mut bytes = std::fs::read(&object).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&object, bytes).unwrap();
    let shallow = archive::export::verify_archives(&pool, &fx.store(), false, None)
        .await
        .unwrap();
    assert_eq!(shallow.ok, 1);
    let deep = archive::export::verify_archives(&pool, &fx.store(), true, None)
        .await
        .unwrap();
    assert_eq!(
        (deep.ok, deep.missing, deep.mismatched),
        (0, 0, 1),
        "{:?}",
        deep.problems
    );

    std::fs::remove_file(&object).unwrap();
    let gone = archive::export::verify_archives(&pool, &fx.store(), false, None)
        .await
        .unwrap();
    assert_eq!((gone.ok, gone.missing, gone.mismatched), (0, 1, 0));

    // A restore with purging off brings the session back but does not
    // purge it again, and says so. Re-export so the object exists again and
    // purge under the enabling configuration, then restore under the
    // shipped one.
    sqlx::query("UPDATE claude_sessions SET archived_at = NULL WHERE session_uuid = $1")
        .bind(fx.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    let cycle = archive::run_cycle(&pool, &fx.purging_config(0), false)
        .await
        .unwrap();
    assert_eq!((cycle.exported, cycle.purged), (1, 1));
    let restored = archive::run_restore(
        &pool,
        &fx.archive_config(0),
        &RestoreScope {
            all: true,
            ..RestoreScope::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(restored.restored, 1);
    assert_eq!(restored.purged_again, 0);
    assert!(restored
        .errors
        .iter()
        .any(|e| e.contains("purging is disabled")));
    assert_eq!(count(&pool, "events", fx.session_uuid).await, 4);
}

#[tokio::test]
async fn durable_dump_uploads_when_a_matching_pg_dump_is_available() {
    let pool = fresh_pool().await;
    let fx = Fixture::new(&pool).await;
    let server_major: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(&pool)
        .await
        .unwrap();
    let server_major: i64 = server_major.parse::<i64>().unwrap() / 10000;
    let client = std::process::Command::new("pg_dump")
        .arg("--version")
        .output();
    let client_major = client
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .last()
                .and_then(|v| v.split('.').next().and_then(|m| m.parse::<i64>().ok()))
        });
    if client_major != Some(server_major) {
        eprintln!(
            "skipping: pg_dump client {client_major:?} does not match server major {server_major}"
        );
        return;
    }
    let dump = archive::dump::dump_durable_tables(&fx.store(), &test_db_url().unwrap())
        .await
        .unwrap();
    assert!(dump.key.starts_with("db/sulion-durable-"));
    assert!(dump.bytes > 0);
    let head = fx
        .store()
        .head(&dump.key)
        .await
        .unwrap()
        .expect("dump object");
    assert_eq!(head.content_length, dump.bytes);
}
