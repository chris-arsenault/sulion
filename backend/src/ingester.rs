//! JSONL ingester. Sole reader of `~/.claude/projects/**/*.jsonl`. All
//! other consumers (REST, WebSocket events) query Postgres — never the
//! files directly.
//!
//! Invariants:
//!   - `ingester_state.last_committed_byte_offset` is ALWAYS at a line
//!     boundary (the byte following a newline, or 0).
//!   - Each tick reads the file from the committed offset to EOF,
//!     processes complete lines (those ending in `\n`), and advances
//!     the offset to past the final newline. A trailing partial line
//!     (no newline) is simply left for the next tick, which will re-read
//!     it from disk once the newline arrives.
//!   - Event rows are keyed on `(session_uuid, byte_offset)` and inserted
//!     with `ON CONFLICT DO NOTHING`, so crash-restarts replay safely.
//!   - Unknown event types are logged and stored with `kind = "unknown"`
//!     — the JSONL format is not a stable public API, and the timeline
//!     can render a generic fallback.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::db::Pool;

/// Heartbeat interval for the "I'm alive, here's what I've done" log.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct IngesterConfig {
    pub projects_dir: PathBuf,
    pub poll_interval: Duration,
}

impl IngesterConfig {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            poll_interval: Duration::from_millis(500),
        }
    }
}

#[derive(Default)]
pub struct Ingester {
    // Cumulative totals since process start. Exposed via the heartbeat
    // log. AtomicU64s because tick() may run concurrently in tests.
    files_seen_total: AtomicU64,
    events_inserted_total: AtomicU64,
    parse_errors_total: AtomicU64,
}

impl Ingester {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cumulative files-seen counter since process start. Read by the
    /// `/api/stats` handler.
    pub fn files_seen_total(&self) -> u64 {
        self.files_seen_total.load(Ordering::Relaxed)
    }

    pub fn events_inserted_total(&self) -> u64 {
        self.events_inserted_total.load(Ordering::Relaxed)
    }

    pub fn parse_errors_total(&self) -> u64 {
        self.parse_errors_total.load(Ordering::Relaxed)
    }

    /// Run continuously. Polls `projects_dir` on `cfg.poll_interval`. Never
    /// returns; callers should `tokio::spawn` it.
    pub async fn run(&self, pool: Pool, cfg: IngesterConfig) {
        // Startup log — confirms the path the ingester will actually
        // watch, so "why aren't my events appearing" has a trivially-
        // visible first answer.
        let projects_exists = cfg.projects_dir.exists();
        tracing::info!(
            projects = %cfg.projects_dir.display(),
            exists = projects_exists,
            poll_ms = cfg.poll_interval.as_millis() as u64,
            "ingester starting",
        );
        if !projects_exists {
            tracing::warn!(
                projects = %cfg.projects_dir.display(),
                "projects directory does not exist yet — ingester will keep polling",
            );
        }

        let mut last_heartbeat = Instant::now();

        loop {
            match self.tick(&pool, &cfg).await {
                Ok(summary) => {
                    if summary.events_inserted > 0 || summary.parse_errors > 0 {
                        tracing::info!(
                            files = summary.files_seen,
                            inserted = summary.events_inserted,
                            parse_errors = summary.parse_errors,
                            "ingester tick summary",
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "ingester tick error");
                }
            }

            if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                tracing::info!(
                    files_seen_total = self.files_seen_total.load(Ordering::Relaxed),
                    events_inserted_total =
                        self.events_inserted_total.load(Ordering::Relaxed),
                    parse_errors_total = self.parse_errors_total.load(Ordering::Relaxed),
                    projects = %cfg.projects_dir.display(),
                    projects_exists = cfg.projects_dir.exists(),
                    "ingester heartbeat",
                );
                last_heartbeat = Instant::now();
            }

            tokio::time::sleep(cfg.poll_interval).await;
        }
    }

    /// Run one pass over every JSONL file in the projects dir. Returns
    /// a summary of what happened this tick. Exposed so tests can drive
    /// the ingester synchronously.
    pub async fn tick(&self, pool: &Pool, cfg: &IngesterConfig) -> anyhow::Result<TickSummary> {
        let mut summary = TickSummary::default();
        if !cfg.projects_dir.exists() {
            return Ok(summary);
        }
        for entry in WalkDir::new(&cfg.projects_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            summary.files_seen += 1;
            self.files_seen_total.fetch_add(1, Ordering::Relaxed);
            match process_file(pool, entry.path()).await {
                Ok(file_result) => {
                    summary.events_inserted += file_result.events_inserted;
                    summary.parse_errors += file_result.parse_errors;
                    self.events_inserted_total
                        .fetch_add(file_result.events_inserted, Ordering::Relaxed);
                    self.parse_errors_total
                        .fetch_add(file_result.parse_errors, Ordering::Relaxed);

                    // Per-file log only when something actually changed.
                    if file_result.events_inserted > 0 {
                        tracing::info!(
                            path = %entry.path().display(),
                            inserted = file_result.events_inserted,
                            parse_errors = file_result.parse_errors,
                            committed_offset = file_result.committed_offset,
                            "ingested events from file",
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        path = %entry.path().display(),
                        %err,
                        "ingest file failed",
                    );
                }
            }
        }
        Ok(summary)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TickSummary {
    pub files_seen: u64,
    pub events_inserted: u64,
    pub parse_errors: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct FileResult {
    events_inserted: u64,
    parse_errors: u64,
    committed_offset: i64,
}

async fn process_file(pool: &Pool, path: &Path) -> anyhow::Result<FileResult> {
    let mut result = FileResult::default();
    let Some(session_uuid) = parse_session_uuid(path) else {
        tracing::debug!(path = %path.display(), "skipping: filename stem is not a uuid");
        return Ok(result);
    };
    let project_hash = parse_project_hash(path);

    upsert_claude_session(pool, session_uuid, project_hash.as_deref()).await?;

    let committed = get_offset(pool, session_uuid).await?;
    let file_len = match std::fs::metadata(path) {
        Ok(md) => md.len() as i64,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "stat failed");
            return Ok(result);
        }
    };

    result.committed_offset = committed;

    if file_len == committed {
        return Ok(result);
    }
    if file_len < committed {
        // File truncated or replaced — reset and try again on next tick.
        tracing::warn!(
            path = %path.display(),
            file_len, committed,
            "file shorter than committed offset; resetting",
        );
        set_offset(pool, session_uuid, path, 0).await?;
        result.committed_offset = 0;
        return Ok(result);
    }

    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(committed as u64))?;
    let mut buf = Vec::with_capacity((file_len - committed) as usize);
    file.read_to_end(&mut buf)?;

    // Walk the buffer. For each newline-terminated line, insert an event
    // and advance `next_committed` past the newline.
    let mut line_start: usize = 0;
    let mut next_committed = committed;

    for (i, &b) in buf.iter().enumerate() {
        if b != b'\n' {
            continue;
        }
        let line = &buf[line_start..i];
        let byte_offset = committed + line_start as i64;
        match insert_event(pool, session_uuid, byte_offset, line).await {
            Ok(inserted) => {
                if inserted {
                    result.events_inserted += 1;
                }
            }
            Err(InsertError::ParseFailed) => {
                result.parse_errors += 1;
            }
            Err(InsertError::Db(err)) => {
                tracing::warn!(%err, byte_offset, "insert_event db failure");
            }
        }
        line_start = i + 1;
        next_committed = committed + line_start as i64;
    }

    // Any tail after the last newline is a partial line. Left in the
    // file; will be re-read on the next tick once it's newline-terminated.

    if next_committed != committed {
        set_offset(pool, session_uuid, path, next_committed).await?;
        result.committed_offset = next_committed;
    }
    Ok(result)
}

enum InsertError {
    ParseFailed,
    Db(sqlx::Error),
}

fn parse_session_uuid(path: &Path) -> Option<Uuid> {
    let stem = path.file_stem()?.to_str()?;
    Uuid::parse_str(stem).ok()
}

fn parse_project_hash(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

async fn upsert_claude_session(
    pool: &Pool,
    session_uuid: Uuid,
    project_hash: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO claude_sessions (session_uuid, project_hash) \
         VALUES ($1, $2) ON CONFLICT (session_uuid) DO NOTHING",
    )
    .bind(session_uuid)
    .bind(project_hash)
    .execute(pool)
    .await?;
    Ok(())
}

async fn get_offset(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<i64> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT last_committed_byte_offset FROM ingester_state WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(o,)| o).unwrap_or(0))
}

async fn set_offset(
    pool: &Pool,
    session_uuid: Uuid,
    path: &Path,
    offset: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO ingester_state (session_uuid, file_path, last_committed_byte_offset, updated_at) \
         VALUES ($1, $2, $3, NOW()) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
           file_path = EXCLUDED.file_path, \
           last_committed_byte_offset = EXCLUDED.last_committed_byte_offset, \
           updated_at = NOW()",
    )
    .bind(session_uuid)
    .bind(path.to_string_lossy().as_ref())
    .bind(offset)
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns Ok(true) if an event row was inserted (i.e. not a dedupe
/// skip); Ok(false) if the line was blank/malformed and silently
/// skipped with the parse-error counter already bumped via Err at the
/// call site.
async fn insert_event(
    pool: &Pool,
    session_uuid: Uuid,
    byte_offset: i64,
    line: &[u8],
) -> Result<bool, InsertError> {
    if line.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(false);
    }

    let value: Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                %err,
                session = %session_uuid,
                byte_offset,
                "malformed JSONL line, skipping",
            );
            return Err(InsertError::ParseFailed);
        }
    };

    let kind = value
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    if kind == "unknown" {
        tracing::debug!(
            session = %session_uuid,
            byte_offset,
            "event without explicit type — stored as 'unknown'",
        );
    }

    // Parse the line into the canonical block representation up-front
    // so the transaction below can write events + event_blocks atomically
    // (same commit). If the parser ever starts failing for a shape it
    // doesn't recognise, we log and fall back to storing the raw row
    // with no blocks — the frontend will render via `unknown` blocks or
    // the legacy payload path.
    use crate::canonical::EventParser;
    let parsed = crate::canonical::ClaudeParser.parse(&value);

    let mut tx = pool.begin().await.map_err(InsertError::Db)?;

    let result = sqlx::query(
        "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         ON CONFLICT (session_uuid, byte_offset) DO NOTHING",
    )
    .bind(session_uuid)
    .bind(byte_offset)
    .bind(timestamp)
    .bind(&kind)
    .bind(&value)
    .bind(parsed.agent)
    .bind(parsed.speaker.as_str())
    .bind(parsed.content_kind.as_str())
    .execute(&mut *tx)
    .await
    .map_err(InsertError::Db)?;

    let inserted = result.rows_affected() > 0;
    if inserted && !parsed.blocks.is_empty() {
        insert_blocks(&mut tx, session_uuid, byte_offset, &parsed.blocks)
            .await
            .map_err(InsertError::Db)?;
    }

    tx.commit().await.map_err(InsertError::Db)?;

    // If this event hints that the current session is a compaction
    // continuation of another, record the parent linkage. Best-effort —
    // we tolerate format drift by checking several field names.
    if let Some(parent) = detect_compaction_parent(&value, session_uuid) {
        if let Err(err) = set_parent_session(pool, session_uuid, parent).await {
            tracing::warn!(%err, session = %session_uuid, parent = %parent, "set_parent_session failed");
        }
    }

    Ok(inserted)
}

async fn insert_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    byte_offset: i64,
    blocks: &[crate::canonical::Block],
) -> sqlx::Result<()> {
    for b in blocks {
        sqlx::query(
            "INSERT INTO event_blocks \
                 (session_uuid, byte_offset, ord, kind, text, \
                  tool_id, tool_name, tool_name_canonical, tool_input, is_error, raw) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             ON CONFLICT (session_uuid, byte_offset, ord) DO NOTHING",
        )
        .bind(session_uuid)
        .bind(byte_offset)
        .bind(b.ord)
        .bind(b.kind.as_str())
        .bind(b.text.as_deref())
        .bind(b.tool_id.as_deref())
        .bind(b.tool_name.as_deref())
        .bind(b.tool_name_canonical.as_deref())
        .bind(b.tool_input.as_ref())
        .bind(b.is_error)
        .bind(b.raw.as_ref())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// One-shot backfill on startup: walk every event that hasn't been
/// decomposed into blocks yet and synthesise them from `payload`. Keeps
/// the frontend's "read from blocks only" invariant true even for rows
/// that pre-date this migration. Cheap because the corpus is small.
pub async fn backfill_canonical_blocks(pool: &Pool) -> anyhow::Result<usize> {
    let rows: Vec<(Uuid, i64, serde_json::Value)> = sqlx::query_as(
        "SELECT e.session_uuid, e.byte_offset, e.payload \
         FROM events e \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM event_blocks b \
              WHERE b.session_uuid = e.session_uuid \
                AND b.byte_offset = e.byte_offset \
         ) \
         AND e.agent = 'claude-code'",
    )
    .fetch_all(pool)
    .await?;

    let count = rows.len();
    if count == 0 {
        return Ok(0);
    }

    use crate::canonical::EventParser;
    let parser = crate::canonical::ClaudeParser;
    for (session_uuid, byte_offset, payload) in rows {
        let parsed = parser.parse(&payload);
        if parsed.blocks.is_empty() {
            continue;
        }
        let mut tx = pool.begin().await?;
        // Update the cached discriminator columns while we're here;
        // older rows may have the default 'claude-code' but NULL for
        // speaker / content_kind.
        sqlx::query(
            "UPDATE events SET speaker = $3, content_kind = $4 \
             WHERE session_uuid = $1 AND byte_offset = $2",
        )
        .bind(session_uuid)
        .bind(byte_offset)
        .bind(parsed.speaker.as_str())
        .bind(parsed.content_kind.as_str())
        .execute(&mut *tx)
        .await?;
        insert_blocks(&mut tx, session_uuid, byte_offset, &parsed.blocks).await?;
        tx.commit().await?;
    }
    Ok(count)
}

/// Scan a JSONL event payload for hints that the current session is a
/// compaction-continuation of another session. Returns the parent uuid
/// when found.
///
/// Claude Code has used different field names over time (`leafUuid`,
/// `parentSessionUuid`, snake_case variants). We accept any of them as
/// long as the referenced uuid is NOT the current session (which would
/// just be a self-reference).
fn detect_compaction_parent(value: &Value, current: Uuid) -> Option<Uuid> {
    // If the event explicitly flags itself as a compact summary, we
    // trust whatever session hint it carries.
    let is_compact = value
        .get("isCompactSummary")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || value
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("summary") || s.eq_ignore_ascii_case("compact_summary"))
            .unwrap_or(false);

    const CANDIDATES: &[&str] = &[
        "leafUuid",
        "parentSessionUuid",
        "parent_session_uuid",
        "parentSessionId",
        "parent_session_id",
        "compactedFromSessionUuid",
    ];
    for key in CANDIDATES {
        if let Some(uuid) = value
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            if uuid != current && (is_compact || key != &"leafUuid") {
                return Some(uuid);
            }
            if is_compact && uuid != current {
                return Some(uuid);
            }
        }
    }
    None
}

async fn set_parent_session(pool: &Pool, session_uuid: Uuid, parent: Uuid) -> anyhow::Result<()> {
    // Only set it once; don't let later events silently overwrite.
    sqlx::query(
        "UPDATE claude_sessions \
         SET parent_session_uuid = $2 \
         WHERE session_uuid = $1 AND parent_session_uuid IS NULL",
    )
    .bind(session_uuid)
    .bind(parent)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_session_uuid_from_filename() {
        let uuid = Uuid::new_v4();
        let path = PathBuf::from(format!("/tmp/abc/{uuid}.jsonl"));
        assert_eq!(parse_session_uuid(&path), Some(uuid));
    }

    #[test]
    fn parse_session_uuid_none_for_non_uuid_stem() {
        let path = PathBuf::from("/tmp/abc/not-a-uuid.jsonl");
        assert_eq!(parse_session_uuid(&path), None);
    }

    #[test]
    fn parse_project_hash_is_parent_dir() {
        let path = PathBuf::from("/tmp/my-project-hash/xxx.jsonl");
        assert_eq!(parse_project_hash(&path), Some("my-project-hash".into()));
    }
}
