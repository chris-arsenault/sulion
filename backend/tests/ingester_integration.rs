#![cfg(feature = "integration-tests")]

//! JSONL ingester integration tests. Exercise the full file-to-Postgres
//! path with synthetic .jsonl fixtures in a tempdir.

use std::io::Write;
use std::path::PathBuf;

use sulion::db;
use sulion::ingest::{rebuild_ingest_derivatives, Ingester, IngesterConfig};
use uuid::Uuid;

const CODEX_RICH_LINEAGE_PARENT: &str = include_str!("fixtures/codex-rich-lineage-parent.jsonl");
const CODEX_RICH_LINEAGE_CHILD: &str = include_str!("fixtures/codex-rich-lineage-child.jsonl");

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    sqlx::query(
        "TRUNCATE events, ingester_state, claude_sessions, pty_sessions, repos, \
         workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .ok();
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

struct Fixture {
    root: tempfile::TempDir,
    project_hash: String,
    session_uuid: Uuid,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let project_hash = "mock-project-hash".to_string();
        let session_uuid = Uuid::new_v4();
        std::fs::create_dir_all(root.path().join(&project_hash)).unwrap();
        Self {
            root,
            project_hash,
            session_uuid,
        }
    }

    fn jsonl_path(&self) -> PathBuf {
        self.root
            .path()
            .join(&self.project_hash)
            .join(format!("{}.jsonl", self.session_uuid))
    }

    fn append(&self, chunk: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.jsonl_path())
            .expect("open for append");
        f.write_all(chunk.as_bytes()).expect("write");
        f.flush().ok();
    }

    fn config(&self) -> IngesterConfig {
        IngesterConfig::new(self.root.path().to_path_buf())
    }
}

struct CodexFixture {
    claude_root: tempfile::TempDir,
    codex_root: tempfile::TempDir,
    session_uuid: Uuid,
}

impl CodexFixture {
    fn new() -> Self {
        let claude_root = tempfile::tempdir().expect("tempdir");
        let codex_root = tempfile::tempdir().expect("tempdir");
        let session_uuid = Uuid::new_v4();
        std::fs::create_dir_all(codex_root.path().join("2026").join("04").join("19")).unwrap();
        Self {
            claude_root,
            codex_root,
            session_uuid,
        }
    }

    fn jsonl_path(&self) -> PathBuf {
        self.codex_root
            .path()
            .join("2026")
            .join("04")
            .join("19")
            .join(format!(
                "rollout-2026-04-19T01-53-43-{}.jsonl",
                self.session_uuid
            ))
    }

    fn append(&self, chunk: &str) {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.jsonl_path())
            .expect("open for append");
        f.write_all(chunk.as_bytes()).expect("write");
        f.flush().ok();
    }

    fn config(&self) -> IngesterConfig {
        IngesterConfig::new(self.claude_root.path().to_path_buf())
            .with_codex_sessions_dir(self.codex_root.path().to_path_buf())
    }
}

async fn event_count(pool: &db::Pool, session: Uuid) -> i64 {
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM events WHERE session_uuid = $1")
            .bind(session)
            .fetch_one(pool)
            .await
            .unwrap();
    n
}

async fn committed_offset(pool: &db::Pool, session: Uuid) -> i64 {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT last_committed_byte_offset FROM ingester_state WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_optional(pool)
    .await
    .unwrap();
    row.map(|(o,)| o).unwrap_or(0)
}

fn codex_rollout_path(root: &std::path::Path, session_uuid: Uuid) -> PathBuf {
    root.join("2026").join("04").join("19").join(format!(
        "rollout-2026-04-19T01-53-43-{}.jsonl",
        session_uuid
    ))
}

#[tokio::test]
async fn ingests_a_simple_event() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z","message":"hi"}"#);
    fx.append("\n");

    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.expect("tick");

    assert_eq!(event_count(&pool, fx.session_uuid).await, 1);
    let kinds: Vec<(String,)> =
        sqlx::query_as("SELECT kind FROM events WHERE session_uuid = $1 ORDER BY byte_offset")
            .bind(fx.session_uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kinds[0].0, "user");
}

#[tokio::test]
async fn ingests_a_codex_rollout_event_from_codex_sessions_dir() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    fx.append(
        r#"{"ts":"2026-04-19T01:53:43.100Z","kind":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello from codex"}]}}"#,
    );
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    assert_eq!(event_count(&pool, fx.session_uuid).await, 1);

    let (agent, project_hash): (String, Option<String>) =
        sqlx::query_as("SELECT agent, project_hash FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(agent, "codex");
    assert!(project_hash.is_none());

    let event: (String, String, String) = sqlx::query_as(
        "SELECT kind, COALESCE(speaker, ''), COALESCE(content_kind, '') \
           FROM events WHERE session_uuid = $1 ORDER BY byte_offset LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event.0, "message");
    assert_eq!(event.1, "assistant");
    assert_eq!(event.2, "text");

    let block: (String, String) = sqlx::query_as(
        "SELECT kind, COALESCE(text, '') \
           FROM event_blocks WHERE session_uuid = $1 ORDER BY byte_offset, ord LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(block.0, "text");
    assert_eq!(block.1, "hello from codex");
}

#[tokio::test]
async fn codex_fixture_preserves_subagent_lineage() {
    let pool = fresh_pool().await;
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    let day_dir = codex_root.path().join("2026").join("04").join("19");
    std::fs::create_dir_all(&day_dir).unwrap();

    let parent = Uuid::parse_str("019da571-ab6d-72e2-94b2-4fc5544f53d2").unwrap();
    let child = Uuid::parse_str("019da789-c2a6-7f80-b71b-4dc90c7f1802").unwrap();
    std::fs::write(
        codex_rollout_path(codex_root.path(), parent),
        CODEX_RICH_LINEAGE_PARENT,
    )
    .unwrap();
    std::fs::write(
        codex_rollout_path(codex_root.path(), child),
        CODEX_RICH_LINEAGE_CHILD,
    )
    .unwrap();

    let cfg = IngesterConfig::new(claude_root.path().to_path_buf())
        .with_codex_sessions_dir(codex_root.path().to_path_buf());
    Ingester::new().tick(&pool, &cfg).await.unwrap();

    let (linked_parent,): (Option<Uuid>,) =
        sqlx::query_as("SELECT parent_session_uuid FROM claude_sessions WHERE session_uuid = $1")
            .bind(child)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked_parent, Some(parent));

    let spawn_edge: (Option<String>, Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain \
           FROM events \
          WHERE session_uuid = $1 AND kind = 'collab_agent_spawn_end' \
          LIMIT 1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        spawn_edge.0.as_deref(),
        Some("019da789-c2a6-7f80-b71b-4dc90c7f1802")
    );
    assert_eq!(
        spawn_edge.1.as_deref(),
        Some("019da788-46e5-7301-b02f-8a2e92f4f50f")
    );
    assert_eq!(
        spawn_edge.2.as_deref(),
        Some("call_P0iOvU7IErNYYbRM5pseMyPT")
    );
    assert!(!spawn_edge.3);

    let spawn_call: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT event_uuid, parent_event_uuid \
           FROM events \
          WHERE session_uuid = $1 AND kind = 'function_call' \
          LIMIT 1",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        spawn_call.0.as_deref(),
        Some("call_P0iOvU7IErNYYbRM5pseMyPT")
    );
    assert_eq!(
        spawn_call.1.as_deref(),
        Some("019da788-46e5-7301-b02f-8a2e92f4f50f")
    );

    let child_turn: (Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT event_uuid, parent_event_uuid, is_sidechain \
           FROM events \
          WHERE session_uuid = $1 AND kind = 'turn_context' \
          LIMIT 1",
    )
    .bind(child)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        child_turn.0.as_deref(),
        Some("019da789-c2f8-7f20-98da-bb832a139ebd")
    );
    assert_eq!(
        child_turn.1.as_deref(),
        Some("019da789-c2a6-7f80-b71b-4dc90c7f1802")
    );
    assert!(child_turn.2);

    let projected_subagents: Vec<(Option<serde_json::Value>,)> = sqlx::query_as(
        "SELECT subagent_json \
           FROM timeline_operations \
          WHERE session_uuid = $1 \
          ORDER BY turn_id, operation_ord",
    )
    .bind(parent)
    .fetch_all(&pool)
    .await
    .unwrap();
    let found_subagent_preview = projected_subagents.into_iter().any(|(subagent_json,)| {
        subagent_json
            .as_ref()
            .and_then(|value| value.get("turns"))
            .and_then(|value| value.as_array())
            .and_then(|turns| turns.first())
            .and_then(|turn| turn.get("preview"))
            .and_then(|value| value.as_str())
            == Some("(assistant) No edits made.")
    });
    assert!(
        found_subagent_preview,
        "parent projection should include child subagent turn"
    );
}

#[tokio::test]
async fn claude_edit_tool_result_payload_is_persisted_canonically() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(
        r#"{"type":"assistant","timestamp":"2025-01-01T00:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_edit_1","name":"Edit","input":{"file_path":"src/lib.rs"}}]}}"#,
    );
    fx.append("\n");
    fx.append(
        r#"{"type":"user","timestamp":"2025-01-01T00:00:01Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_edit_1","content":"","is_error":false}]},"toolUseResult":{"filePath":"src/lib.rs","oldString":"fn old() {}\n","newString":"fn new() {}\n","replaceAll":false,"structuredPatch":[{"oldString":"fn old() {}\n","newString":"fn new() {}\n"}]}}"#,
    );
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let tool_output: serde_json::Value = sqlx::query_scalar(
        "SELECT tool_output \
           FROM event_blocks \
          WHERE session_uuid = $1 AND kind = 'tool_result' \
          LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tool_output["path"], "src/lib.rs");
    assert_eq!(tool_output["old_text"], "fn old() {}\n");
    assert_eq!(tool_output["new_text"], "fn new() {}\n");
    assert_eq!(tool_output["replace_all"], false);

    let result_payload: serde_json::Value = sqlx::query_scalar(
        "SELECT result_payload \
           FROM timeline_operations \
          WHERE session_uuid = $1 AND pair_id = 'toolu_edit_1' \
          LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result_payload["path"], "src/lib.rs");
    assert_eq!(result_payload["old_text"], "fn old() {}\n");
    assert_eq!(result_payload["new_text"], "fn new() {}\n");
}

#[tokio::test]
async fn append_rebuilds_only_the_affected_projection_suffix() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(
        r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":"first prompt"}}"#,
    );
    fx.append("\n");
    fx.append(
        r#"{"type":"assistant","timestamp":"2025-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"first answer"}]}}"#,
    );
    fx.append("\n");
    fx.append(
        r#"{"type":"user","timestamp":"2025-01-01T00:00:02Z","message":{"role":"user","content":"second prompt"}}"#,
    );
    fx.append("\n");
    fx.append(
        r#"{"type":"assistant","timestamp":"2025-01-01T00:00:03Z","message":{"role":"assistant","content":[{"type":"text","text":"second answer"}]}}"#,
    );
    fx.append("\n");

    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let turns: Vec<(i64, i32, String, i32)> = sqlx::query_as(
        "SELECT turn_id, turn_ord, preview, event_count \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
          ORDER BY turn_ord ASC",
    )
    .bind(fx.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].2, "first prompt");
    assert_eq!(turns[1].2, "second prompt");

    sqlx::query(
        "UPDATE timeline_turns \
            SET preview = 'sentinel-first-turn-was-not-rebuilt' \
          WHERE session_uuid = $1 AND turn_id = $2",
    )
    .bind(fx.session_uuid)
    .bind(turns[0].0)
    .execute(&pool)
    .await
    .unwrap();

    fx.append(
        r#"{"type":"assistant","timestamp":"2025-01-01T00:00:04Z","message":{"role":"assistant","content":[{"type":"text","text":"second follow-up"}]}}"#,
    );
    fx.append("\n");
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let updated: Vec<(i64, i32, String, i32)> = sqlx::query_as(
        "SELECT turn_id, turn_ord, preview, event_count \
           FROM timeline_turns \
          WHERE session_uuid = $1 \
          ORDER BY turn_ord ASC",
    )
    .bind(fx.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(updated.len(), 2);
    assert_eq!(updated[0].0, turns[0].0);
    assert_eq!(updated[0].1, 0);
    assert_eq!(updated[0].2, "sentinel-first-turn-was-not-rebuilt");
    assert_eq!(updated[1].0, turns[1].0);
    assert_eq!(updated[1].1, 1);
    assert_eq!(updated[1].2, "second prompt");
    assert_eq!(updated[1].3, 3);
}

#[tokio::test]
async fn clean_files_are_filtered_before_session_upsert() {
    let pool = fresh_pool().await;
    let root = tempfile::tempdir().expect("tempdir");
    let project_hash = "mock-project-hash".to_string();
    std::fs::create_dir_all(root.path().join(&project_hash)).unwrap();
    let clean_session = Uuid::new_v4();
    let dirty_session = Uuid::new_v4();
    let clean_path = root
        .path()
        .join(&project_hash)
        .join(format!("{clean_session}.jsonl"));
    let dirty_path = root
        .path()
        .join(&project_hash)
        .join(format!("{dirty_session}.jsonl"));
    std::fs::write(
        &clean_path,
        r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z","message":{"role":"user","content":"clean"}}"#,
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&clean_path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();

    let ingester = Ingester::new();
    let cfg = IngesterConfig::new(root.path().to_path_buf());
    ingester.tick(&pool, &cfg).await.unwrap();
    assert_eq!(event_count(&pool, clean_session).await, 1);

    sqlx::query("UPDATE claude_sessions SET agent = 'sentinel-agent' WHERE session_uuid = $1")
        .bind(clean_session)
        .execute(&pool)
        .await
        .unwrap();
    std::fs::write(
        &dirty_path,
        r#"{"type":"user","timestamp":"2025-01-01T00:00:01Z","message":{"role":"user","content":"dirty"}}"#,
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&dirty_path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();

    ingester.tick(&pool, &cfg).await.unwrap();

    let (agent,): (String,) =
        sqlx::query_as("SELECT agent FROM claude_sessions WHERE session_uuid = $1")
            .bind(clean_session)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(agent, "sentinel-agent");
    assert_eq!(event_count(&pool, dirty_session).await, 1);
}

#[tokio::test]
async fn partial_trailing_line_is_not_committed() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    // One complete line, followed by a partial (no trailing newline).
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");
    fx.append(r#"{"type":"assistant","timestamp":"2025-01-01T00:00:01Z"#);

    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(
        event_count(&pool, fx.session_uuid).await,
        1,
        "partial line must not be ingested"
    );

    // Complete the partial line; next tick picks it up.
    fx.append(r#""}"#);
    fx.append("\n");

    ingester.tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(
        event_count(&pool, fx.session_uuid).await,
        2,
        "newly-completed line should be ingested on the next tick"
    );
}

#[tokio::test]
async fn unknown_event_type_is_stored_with_unknown_kind() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(r#"{"type":"new_event_type_from_the_future","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");
    fx.append(r#"{"no_type_field":"oops","timestamp":"2025-01-01T00:00:01Z"}"#);
    fx.append("\n");

    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let kinds: Vec<(String,)> =
        sqlx::query_as("SELECT kind FROM events WHERE session_uuid = $1 ORDER BY byte_offset")
            .bind(fx.session_uuid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kinds.len(), 2);
    // First keeps its (unknown-to-us) kind; second stored as "unknown".
    assert_eq!(kinds[0].0, "new_event_type_from_the_future");
    assert_eq!(kinds[1].0, "unknown");
}

#[tokio::test]
async fn malformed_line_is_skipped_without_stalling() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");
    fx.append("this is not json\n");
    fx.append(r#"{"type":"assistant","timestamp":"2025-01-01T00:00:02Z"}"#);
    fx.append("\n");

    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();

    assert_eq!(event_count(&pool, fx.session_uuid).await, 2);
    let committed = committed_offset(&pool, fx.session_uuid).await;
    let file_len = std::fs::metadata(fx.jsonl_path()).unwrap().len() as i64;
    assert_eq!(
        committed, file_len,
        "offset must have advanced past the malformed line"
    );
}

#[tokio::test]
async fn restart_resumes_from_committed_offset_without_duplicates() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    for i in 0..5 {
        fx.append(&format!(
            r#"{{"type":"user","timestamp":"2025-01-01T00:00:0{i}Z","n":{i}}}"#
        ));
        fx.append("\n");
    }

    let first = Ingester::new();
    first.tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(event_count(&pool, fx.session_uuid).await, 5);

    // Simulate a restart: fresh ingester instance, same DB.
    let second = Ingester::new();
    second.tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(
        event_count(&pool, fx.session_uuid).await,
        5,
        "restart must not re-insert already-committed events",
    );

    // Append more, tick again, verify only new ones added.
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:09Z","n":9}"#);
    fx.append("\n");

    second.tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(event_count(&pool, fx.session_uuid).await, 6);
}

#[tokio::test]
async fn claude_sessions_row_is_created_with_project_hash() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let row: Option<(String,)> =
        sqlx::query_as("SELECT project_hash FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(row.unwrap().0, fx.project_hash);
}

#[tokio::test]
async fn reindex_preserves_correlated_terminal_association() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state) \
         VALUES ($1, 'repo-a', '/tmp/repo-a', 'live')",
    )
    .bind(pty_id)
    .execute(&pool)
    .await
    .unwrap();
    sulion::correlate::apply(
        &pool,
        &sulion::correlate::CorrelateMsg {
            pty_id,
            session_uuid: fx.session_uuid,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(event_count(&pool, fx.session_uuid).await, 1);

    let stats = rebuild_ingest_derivatives(&pool).await.unwrap();
    assert_eq!(stats.sessions_rebuilt, 1);
    assert_eq!(stats.events_preserved, 1);
    assert_eq!(stats.canonical_events_rebuilt, 1);
    assert_eq!(stats.timeline_sessions_rebuilt, 1);
    assert_eq!(event_count(&pool, fx.session_uuid).await, 1);

    let reset_row: (Option<Uuid>, Option<String>, Option<Uuid>) = sqlx::query_as(
        "SELECT pty_session_id, project_hash, parent_session_uuid \
           FROM claude_sessions \
          WHERE session_uuid = $1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reset_row.0, Some(pty_id));
    assert_eq!(reset_row.1.as_deref(), Some(fx.project_hash.as_str()));
    assert!(reset_row.2.is_none());

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let replayed_row: (Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT pty_session_id, project_hash \
           FROM claude_sessions \
          WHERE session_uuid = $1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event_count(&pool, fx.session_uuid).await, 1);
    assert_eq!(replayed_row.0, Some(pty_id));
    assert_eq!(replayed_row.1.as_deref(), Some(fx.project_hash.as_str()));
}

#[tokio::test]
async fn ingest_restores_current_pty_link_when_session_row_was_lost() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions \
         (id, repo, working_dir, state, current_claude_session_uuid, current_session_uuid, current_session_agent) \
         VALUES ($1, 'repo-a', '/tmp/repo-a', 'live', $2, $2, 'claude-code')",
    )
    .bind(pty_id)
    .bind(fx.session_uuid)
    .execute(&pool)
    .await
    .unwrap();
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let (linked_pty,): (Option<Uuid>,) =
        sqlx::query_as("SELECT pty_session_id FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked_pty, Some(pty_id));
}

#[tokio::test]
async fn non_uuid_filename_is_skipped() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let bogus_path = fx
        .root
        .path()
        .join(&fx.project_hash)
        .join("not-a-uuid.jsonl");
    std::fs::write(
        &bogus_path,
        r#"{"type":"user"}
"#,
    )
    .unwrap();

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "non-uuid filename should be skipped entirely");
}

#[tokio::test]
async fn compaction_event_links_parent_session_uuid() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let parent = Uuid::new_v4();
    // First event in a compacted session flags itself as a summary and
    // carries the prior session's uuid.
    let line = format!(
        r#"{{"type":"summary","timestamp":"2025-01-01T00:00:00Z","isCompactSummary":true,"leafUuid":"{parent}"}}"#
    );
    fx.append(&line);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let (linked,): (Option<Uuid>,) =
        sqlx::query_as("SELECT parent_session_uuid FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked, Some(parent));
}

#[tokio::test]
async fn compaction_linkage_uses_parent_session_uuid_field_too() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let parent = Uuid::new_v4();
    let line = format!(
        r#"{{"type":"user","timestamp":"2025-01-01T00:00:00Z","parentSessionUuid":"{parent}"}}"#
    );
    fx.append(&line);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let (linked,): (Option<Uuid>,) =
        sqlx::query_as("SELECT parent_session_uuid FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked, Some(parent));
}

#[tokio::test]
async fn compaction_linkage_ignored_when_self_referencing() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    // An event with parentSessionUuid equal to the current session shouldn't
    // create a self-link.
    let self_uuid = fx.session_uuid;
    let line = format!(
        r#"{{"type":"user","timestamp":"2025-01-01T00:00:00Z","parentSessionUuid":"{self_uuid}"}}"#
    );
    fx.append(&line);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();

    let (linked,): (Option<Uuid>,) =
        sqlx::query_as("SELECT parent_session_uuid FROM claude_sessions WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(linked.is_none());
}

#[tokio::test]
async fn file_truncation_resets_offset() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    fx.append(r#"{"type":"user","timestamp":"2025-01-01T00:00:00Z"}"#);
    fx.append("\n");

    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(event_count(&pool, fx.session_uuid).await, 1);
    let first_offset = committed_offset(&pool, fx.session_uuid).await;
    assert!(first_offset > 0);

    // Truncate + replace with a smaller content.
    std::fs::write(fx.jsonl_path(), "").unwrap();

    // First tick after truncation resets the offset; second tick re-ingests.
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(committed_offset(&pool, fx.session_uuid).await, 0);
}
