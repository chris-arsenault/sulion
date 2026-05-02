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
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::db::Pool;

use super::file_scan::{dirty_transcript_files, DirtyTranscriptFile};

/// Heartbeat interval for the "I'm alive, here's what I've done" log.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct IngesterConfig {
    pub claude_projects_dir: PathBuf,
    pub codex_sessions_dir: Option<PathBuf>,
    pub poll_interval: Duration,
}

impl IngesterConfig {
    pub fn new(claude_projects_dir: PathBuf) -> Self {
        Self {
            claude_projects_dir,
            codex_sessions_dir: None,
            poll_interval: Duration::from_millis(500),
        }
    }

    pub fn with_codex_sessions_dir(mut self, codex_sessions_dir: PathBuf) -> Self {
        self.codex_sessions_dir = Some(codex_sessions_dir);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TranscriptSource {
    ClaudeCode,
    Codex,
}

impl TranscriptSource {
    pub(super) fn agent_id(self) -> &'static str {
        match self {
            TranscriptSource::ClaudeCode => "claude-code",
            TranscriptSource::Codex => "codex",
        }
    }
}

#[derive(Default)]
pub struct Ingester {
    // Cumulative totals since process start. Exposed via the heartbeat
    // log. AtomicU64s because tick() may run concurrently in tests.
    events_inserted_total: AtomicU64,
    parse_errors_total: AtomicU64,
    last_tick_started_at_unix: AtomicI64,
    last_progress_at_unix: AtomicI64,
}

impl Ingester {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events_inserted_total(&self) -> u64 {
        self.events_inserted_total.load(Ordering::Relaxed)
    }

    pub fn parse_errors_total(&self) -> u64 {
        self.parse_errors_total.load(Ordering::Relaxed)
    }

    pub fn last_tick_started_at_unix(&self) -> Option<i64> {
        unix_timestamp_from_atomic(&self.last_tick_started_at_unix)
    }

    pub fn last_progress_at_unix(&self) -> Option<i64> {
        unix_timestamp_from_atomic(&self.last_progress_at_unix)
    }

    /// Run continuously. Polls `projects_dir` on `cfg.poll_interval`. Never
    /// returns; callers should `tokio::spawn` it.
    pub async fn run(&self, pool: Pool, cfg: IngesterConfig) {
        // Startup log — confirms the path the ingester will actually
        // watch, so "why aren't my events appearing" has a trivially-
        // visible first answer.
        let claude_exists = cfg.claude_projects_dir.exists();
        let codex_exists = cfg
            .codex_sessions_dir
            .as_ref()
            .map(|p| p.exists())
            .unwrap_or(false);
        tracing::info!(
            claude_projects = %cfg.claude_projects_dir.display(),
            claude_exists,
            codex_sessions = %cfg
                .codex_sessions_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(disabled)".to_string()),
            codex_exists,
            poll_ms = cfg.poll_interval.as_millis() as u64,
            "ingester starting",
        );
        if !claude_exists {
            tracing::warn!(
                projects = %cfg.claude_projects_dir.display(),
                "Claude projects directory does not exist yet — ingester will keep polling",
            );
        }
        if let Some(codex_dir) = &cfg.codex_sessions_dir {
            if !codex_exists {
                tracing::warn!(
                    sessions = %codex_dir.display(),
                    "Codex sessions directory does not exist yet — ingester will keep polling",
                );
            }
        }

        let mut last_heartbeat = Instant::now();

        loop {
            self.last_tick_started_at_unix
                .store(Utc::now().timestamp(), Ordering::Relaxed);
            match self.tick(&pool, &cfg).await {
                Ok(summary) => {
                    if summary.events_inserted > 0 || summary.parse_errors > 0 {
                        self.last_progress_at_unix
                            .store(Utc::now().timestamp(), Ordering::Relaxed);
                        tracing::info!(
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
                        events_inserted_total =
                            self.events_inserted_total.load(Ordering::Relaxed),
                        parse_errors_total = self.parse_errors_total.load(Ordering::Relaxed),
                    claude_projects = %cfg.claude_projects_dir.display(),
                    claude_exists = cfg.claude_projects_dir.exists(),
                    codex_sessions = %cfg
                        .codex_sessions_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(disabled)".to_string()),
                    codex_exists = cfg
                        .codex_sessions_dir
                        .as_ref()
                        .map(|p| p.exists())
                        .unwrap_or(false),
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
        self.tick_root(
            pool,
            &cfg.claude_projects_dir,
            TranscriptSource::ClaudeCode,
            &mut summary,
        )
        .await;
        if let Some(codex_dir) = &cfg.codex_sessions_dir {
            self.tick_root(pool, codex_dir, TranscriptSource::Codex, &mut summary)
                .await;
        }
        Ok(summary)
    }

    async fn tick_root(
        &self,
        pool: &Pool,
        root: &Path,
        source: TranscriptSource,
        summary: &mut TickSummary,
    ) {
        if !root.exists() {
            return;
        }
        let dirty_files = match dirty_transcript_files(pool, root, source).await {
            Ok(files) => files,
            Err(err) => {
                tracing::warn!(
                    agent = source.agent_id(),
                    root = %root.display(),
                    %err,
                    "ingest root scan failed",
                );
                return;
            }
        };
        for file in dirty_files {
            match process_file(pool, &file, source).await {
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
                            agent = source.agent_id(),
                            path = %file.path.display(),
                            inserted = file_result.events_inserted,
                            parse_errors = file_result.parse_errors,
                            committed_offset = file_result.committed_offset,
                            "ingested events from file",
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        agent = source.agent_id(),
                        path = %file.path.display(),
                        %err,
                        "ingest file failed",
                    );
                }
            }
        }
    }
}

fn unix_timestamp_from_atomic(value: &AtomicI64) -> Option<i64> {
    let raw = value.load(Ordering::Relaxed);
    (raw > 0).then_some(raw)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TickSummary {
    pub events_inserted: u64,
    pub parse_errors: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct FileResult {
    events_inserted: u64,
    parse_errors: u64,
    committed_offset: i64,
}

#[derive(Debug, Clone)]
struct CodexSessionContext {
    session_id: String,
    parent_session_id: Option<String>,
    current_turn_id: Option<String>,
}

impl CodexSessionContext {
    fn new(session_uuid: Uuid) -> Self {
        Self {
            session_id: session_uuid.to_string(),
            parent_session_id: None,
            current_turn_id: None,
        }
    }
}

async fn process_file(
    pool: &Pool,
    file: &DirtyTranscriptFile,
    source: TranscriptSource,
) -> anyhow::Result<FileResult> {
    let mut result = FileResult::default();
    upsert_agent_session(
        pool,
        file.session_uuid,
        source.agent_id(),
        file.project_hash.as_deref(),
    )
    .await?;

    result.committed_offset = file.committed_offset;
    if file.file_len < file.committed_offset {
        // File truncated or replaced — reset and try again on next tick.
        tracing::warn!(
            path = %file.path.display(),
            file_len = file.file_len,
            committed = file.committed_offset,
            "file shorter than committed offset; resetting",
        );
        set_offset(pool, file.session_uuid, &file.path, 0).await?;
        result.committed_offset = 0;
        return Ok(result);
    }

    let mut transcript = std::fs::File::open(&file.path)?;
    transcript.seek(SeekFrom::Start(file.committed_offset as u64))?;
    let mut buf = Vec::with_capacity((file.file_len - file.committed_offset) as usize);
    transcript.read_to_end(&mut buf)?;

    // Walk the buffer. For each newline-terminated line, insert an event
    // and advance `next_committed` past the newline.
    let mut line_start: usize = 0;
    let mut next_committed = file.committed_offset;
    let mut first_inserted_offset: Option<i64> = None;
    let mut codex_ctx = match source {
        TranscriptSource::Codex => Some(load_codex_context(pool, file.session_uuid).await?),
        TranscriptSource::ClaudeCode => None,
    };

    for (i, &b) in buf.iter().enumerate() {
        if b != b'\n' {
            continue;
        }
        let line = &buf[line_start..i];
        let byte_offset = file.committed_offset + line_start as i64;
        match insert_event(
            pool,
            file.session_uuid,
            source,
            byte_offset,
            line,
            codex_ctx.as_mut(),
        )
        .await
        {
            Ok(inserted) => {
                if inserted {
                    first_inserted_offset.get_or_insert(byte_offset);
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
        next_committed = file.committed_offset + line_start as i64;
    }

    // Any tail after the last newline is a partial line. Left in the
    // file; will be re-read on the next tick once it's newline-terminated.

    if next_committed != file.committed_offset {
        set_offset(pool, file.session_uuid, &file.path, next_committed).await?;
        result.committed_offset = next_committed;
    }
    if let Some(first_inserted_offset) = first_inserted_offset {
        rebuild_projections_after_insert(pool, file.session_uuid, source, first_inserted_offset)
            .await;
    }
    Ok(result)
}

async fn rebuild_projections_after_insert(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    first_inserted_offset: i64,
) {
    if let Err(err) = super::projection::rebuild_session_projection_after_insert(
        pool,
        session_uuid,
        first_inserted_offset,
    )
    .await
    {
        tracing::warn!(
            %err,
            session = %session_uuid,
            agent = source.agent_id(),
            "timeline projection rebuild failed",
        );
    }
    if source == TranscriptSource::Codex {
        rebuild_codex_ancestor_projections(pool, session_uuid).await;
    }
}

async fn rebuild_codex_ancestor_projections(pool: &Pool, session_uuid: Uuid) {
    match codex_parent_session_chain(pool, session_uuid).await {
        Ok(ancestors) => {
            for ancestor in ancestors {
                if let Err(err) =
                    super::projection::rebuild_session_projection(pool, ancestor).await
                {
                    tracing::warn!(
                        %err,
                        session = %session_uuid,
                        ancestor = %ancestor,
                        "ancestor timeline projection rebuild failed",
                    );
                }
            }
        }
        Err(err) => {
            tracing::warn!(
                %err,
                session = %session_uuid,
                "codex ancestor projection lookup failed",
            );
        }
    }
}

enum InsertError {
    ParseFailed,
    Db(sqlx::Error),
}

pub(super) fn parse_session_uuid(path: &Path, source: TranscriptSource) -> Option<Uuid> {
    match source {
        TranscriptSource::ClaudeCode => parse_claude_session_uuid(path),
        TranscriptSource::Codex => parse_codex_session_uuid(path),
    }
}

fn parse_claude_session_uuid(path: &Path) -> Option<Uuid> {
    let stem = path.file_stem()?.to_str()?;
    Uuid::parse_str(stem).ok()
}

pub fn parse_codex_session_uuid(path: &Path) -> Option<Uuid> {
    let stem = path.file_stem()?.to_str()?;
    let start = stem.len().checked_sub(36)?;
    let raw = stem.get(start..)?;
    Uuid::parse_str(raw).ok()
}

pub(super) fn parse_project_hash(path: &Path, source: TranscriptSource) -> Option<String> {
    match source {
        TranscriptSource::ClaudeCode => path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string()),
        TranscriptSource::Codex => None,
    }
}

async fn upsert_agent_session(
    pool: &Pool,
    session_uuid: Uuid,
    agent: &str,
    project_hash: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO claude_sessions (session_uuid, agent, project_hash, pty_session_id) \
         VALUES ( \
           $1, \
           $2, \
           $3, \
           (SELECT id FROM pty_sessions WHERE current_session_uuid = $1 LIMIT 1) \
         ) \
         ON CONFLICT (session_uuid) DO UPDATE SET \
           agent = EXCLUDED.agent, \
           project_hash = COALESCE(EXCLUDED.project_hash, claude_sessions.project_hash), \
           pty_session_id = COALESCE(claude_sessions.pty_session_id, EXCLUDED.pty_session_id)",
    )
    .bind(session_uuid)
    .bind(agent)
    .bind(project_hash)
    .execute(pool)
    .await?;
    Ok(())
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

async fn load_codex_context(
    pool: &Pool,
    session_uuid: Uuid,
) -> anyhow::Result<CodexSessionContext> {
    let mut ctx = CodexSessionContext::new(session_uuid);

    let meta_row: Option<(Value,)> = sqlx::query_as(
        "SELECT payload \
         FROM events \
         WHERE session_uuid = $1 AND agent = 'codex' AND kind = 'session_meta' \
         ORDER BY byte_offset DESC \
         LIMIT 1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    if let Some((payload,)) = meta_row {
        update_codex_context(&mut ctx, &payload, session_uuid);
    }

    let turn_row: Option<(Value,)> = sqlx::query_as(
        "SELECT payload \
         FROM events \
         WHERE session_uuid = $1 AND agent = 'codex' AND kind IN ('turn_context', 'task_started') \
         ORDER BY byte_offset DESC \
         LIMIT 1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    if let Some((payload,)) = turn_row {
        update_codex_context(&mut ctx, &payload, session_uuid);
    }

    Ok(ctx)
}

async fn codex_parent_session_chain(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<Vec<Uuid>> {
    let mut chain = Vec::new();
    let mut current = session_uuid;
    loop {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT parent_session_uuid \
               FROM claude_sessions \
              WHERE session_uuid = $1 AND parent_session_uuid IS NOT NULL",
        )
        .bind(current)
        .fetch_optional(pool)
        .await?;
        let Some((parent,)) = row else {
            break;
        };
        chain.push(parent);
        current = parent;
    }
    Ok(chain)
}

/// Returns Ok(true) if an event row was inserted (i.e. not a dedupe
/// skip); Ok(false) if the line was blank/malformed and silently
/// skipped with the parse-error counter already bumped via Err at the
/// call site.
async fn insert_event(
    pool: &Pool,
    session_uuid: Uuid,
    source: TranscriptSource,
    byte_offset: i64,
    line: &[u8],
    codex_ctx: Option<&mut CodexSessionContext>,
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

    // Parse the line into the canonical block representation up-front
    // so the transaction below can write events + event_blocks atomically
    // (same commit). If the parser ever starts failing for a shape it
    // doesn't recognise, we log and fall back to storing the raw row
    // with no blocks — the frontend will render via `unknown` blocks or
    // the legacy payload path.
    let codex_ctx_ref = codex_ctx.as_deref();
    let parsed = parse_canonical_event(
        source.agent_id(),
        &value,
        session_uuid,
        byte_offset,
        codex_ctx_ref,
    );
    let kind = stored_event_kind(source, &value, &parsed);
    let timestamp = parse_event_timestamp(&value).unwrap_or_else(Utc::now);

    if kind == "unknown" {
        tracing::debug!(
            session = %session_uuid,
            byte_offset,
            agent = source.agent_id(),
            "event without explicit type — stored as 'unknown'",
        );
    }

    let search_text = parsed.search_text();
    let mut tx = pool.begin().await.map_err(InsertError::Db)?;

    let result = sqlx::query(
        "INSERT INTO events \
             (session_uuid, byte_offset, timestamp, kind, payload, agent, speaker, content_kind, \
              event_uuid, parent_event_uuid, related_tool_use_id, is_sidechain, is_meta, subtype, search_text) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) \
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
    .bind(parsed.event_uuid.as_deref())
    .bind(parsed.parent_event_uuid.as_deref())
    .bind(parsed.related_tool_use_id.as_deref())
    .bind(parsed.is_sidechain)
    .bind(parsed.is_meta)
    .bind(parsed.subtype.as_deref())
    .bind(&search_text)
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

    if let Some(ctx) = codex_ctx {
        update_codex_context(ctx, &value, session_uuid);
    }

    // If this event hints that the current session is a compaction
    // continuation of another, record the parent linkage. Best-effort —
    // we tolerate format drift by checking several field names.
    if source == TranscriptSource::ClaudeCode {
        if let Some(parent) = detect_compaction_parent(&value, session_uuid) {
            if let Err(err) = set_parent_session(pool, session_uuid, parent).await {
                tracing::warn!(%err, session = %session_uuid, parent = %parent, "set_parent_session failed");
            }
        }
    }
    if source == TranscriptSource::Codex {
        if let Some(parent) = detect_codex_parent_session(&value, session_uuid) {
            if let Err(err) = set_parent_session(pool, session_uuid, parent).await {
                tracing::warn!(%err, session = %session_uuid, parent = %parent, "set_parent_session failed");
            }
        }
    }

    Ok(inserted)
}

fn parse_canonical_event(
    agent: &str,
    value: &Value,
    session_uuid: Uuid,
    byte_offset: i64,
    codex_ctx: Option<&CodexSessionContext>,
) -> super::canonical::CanonicalEvent {
    use super::canonical::EventParser;
    let mut parsed = match agent {
        "codex" => super::canonical::CodexParser.parse(value),
        _ => super::canonical::ClaudeParser.parse(value),
    };
    if agent == "codex" {
        enrich_codex_lineage(&mut parsed, value, session_uuid, byte_offset, codex_ctx);
    }
    parsed
}

fn stored_event_kind(
    source: TranscriptSource,
    value: &Value,
    parsed: &super::canonical::CanonicalEvent,
) -> String {
    match source {
        TranscriptSource::ClaudeCode => value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        TranscriptSource::Codex => {
            let outer = super::canonical::codex_record_kind(value).unwrap_or("");
            let subtype = parsed
                .subtype
                .as_deref()
                .filter(|kind| !kind.is_empty())
                .unwrap_or("unknown");
            match outer {
                "response_item" | "event_msg" => subtype.to_string(),
                "" => subtype.to_string(),
                _ => outer.to_string(),
            }
        }
    }
}

fn enrich_codex_lineage(
    parsed: &mut super::canonical::CanonicalEvent,
    value: &Value,
    session_uuid: Uuid,
    byte_offset: i64,
    codex_ctx: Option<&CodexSessionContext>,
) {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    let outer = super::canonical::codex_record_kind(value).unwrap_or("");
    let subtype = parsed.subtype.as_deref().unwrap_or("");
    let session_id = codex_ctx
        .map(|ctx| ctx.session_id.clone())
        .unwrap_or_else(|| session_uuid.to_string());
    let session_parent = codex_ctx.and_then(|ctx| ctx.parent_session_id.clone());
    let current_turn_id = codex_ctx.and_then(|ctx| ctx.current_turn_id.clone());
    let synthetic_id = format!("codex:{session_uuid}:{byte_offset}");

    match outer {
        "session_meta" => {
            parsed.event_uuid = codex_string_at_path(payload, &["id"])
                .map(ToString::to_string)
                .or(Some(session_id.clone()));
            parsed.parent_event_uuid = codex_parent_session_string(value).map(ToString::to_string);
            parsed.is_sidechain = parsed.parent_event_uuid.is_some();
        }
        "turn_context" => {
            parsed.event_uuid = codex_string_at_path(payload, &["turn_id"])
                .map(ToString::to_string)
                .or(Some(synthetic_id));
            parsed.parent_event_uuid = Some(session_id);
            parsed.is_sidechain = session_parent.is_some();
        }
        "response_item" => {
            let parent = current_turn_id.or(Some(session_id));
            parsed.parent_event_uuid = parent;
            parsed.is_sidechain = session_parent.is_some();
            parsed.event_uuid = match subtype {
                "function_call" | "custom_tool_call" => parsed
                    .blocks
                    .iter()
                    .find_map(|block| block.tool_id.clone())
                    .or(Some(synthetic_id)),
                "function_call_output" | "custom_tool_call_output" => parsed
                    .related_tool_use_id
                    .as_ref()
                    .map(|id| format!("{id}:output:{byte_offset}"))
                    .or(Some(synthetic_id)),
                _ => Some(synthetic_id),
            };
        }
        "event_msg" => {
            parsed.related_tool_use_id = parsed
                .related_tool_use_id
                .clone()
                .or_else(|| codex_string_at_path(payload, &["call_id"]).map(ToString::to_string));
            parsed.is_sidechain = session_parent.is_some();
            match subtype {
                "task_started" => {
                    parsed.event_uuid = codex_string_at_path(payload, &["turn_id"])
                        .map(ToString::to_string)
                        .or(Some(synthetic_id));
                    parsed.parent_event_uuid = Some(session_id);
                }
                "collab_agent_spawn_end" => {
                    parsed.event_uuid = codex_string_at_path(payload, &["new_thread_id"])
                        .map(ToString::to_string)
                        .or(Some(synthetic_id));
                    parsed.parent_event_uuid = current_turn_id.or(Some(session_id));
                    parsed.is_sidechain = false;
                }
                _ => {
                    parsed.event_uuid = Some(synthetic_id);
                    parsed.parent_event_uuid = codex_string_at_path(payload, &["turn_id"])
                        .map(ToString::to_string)
                        .or(current_turn_id)
                        .or(Some(session_id));
                }
            }
        }
        _ => {}
    }
}

fn update_codex_context(ctx: &mut CodexSessionContext, value: &Value, session_uuid: Uuid) {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    match super::canonical::codex_record_kind(value).unwrap_or("") {
        "session_meta" => {
            ctx.session_id = codex_string_at_path(payload, &["id"])
                .map(ToString::to_string)
                .unwrap_or_else(|| session_uuid.to_string());
            ctx.parent_session_id = codex_parent_session_string(value).map(ToString::to_string);
        }
        "turn_context" => {
            ctx.current_turn_id =
                codex_string_at_path(payload, &["turn_id"]).map(ToString::to_string);
        }
        "event_msg" if payload.get("type").and_then(|v| v.as_str()) == Some("task_started") => {
            ctx.current_turn_id =
                codex_string_at_path(payload, &["turn_id"]).map(ToString::to_string);
        }
        _ => {}
    }
}

fn codex_parent_session_string(value: &Value) -> Option<&str> {
    let payload = value.get("payload").unwrap_or(&Value::Null);
    codex_string_at_path(payload, &["forked_from_id"]).or_else(|| {
        codex_string_at_path(
            payload,
            &["source", "subagent", "thread_spawn", "parent_thread_id"],
        )
    })
}

fn detect_codex_parent_session(value: &Value, current: Uuid) -> Option<Uuid> {
    codex_parent_session_string(value)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .filter(|uuid| *uuid != current)
}

fn codex_string_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current.as_str()
}

fn parse_event_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("timestamp")
        .or_else(|| value.get("ts"))
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

async fn insert_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session_uuid: Uuid,
    byte_offset: i64,
    blocks: &[super::canonical::Block],
) -> sqlx::Result<()> {
    for b in blocks {
        sqlx::query(
            "INSERT INTO event_blocks \
                 (session_uuid, byte_offset, ord, kind, text, \
                  tool_id, tool_name, tool_name_canonical, tool_input, tool_output, is_error, raw) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (session_uuid, byte_offset, ord) DO UPDATE SET \
                 kind = EXCLUDED.kind, \
                 text = EXCLUDED.text, \
                 tool_id = EXCLUDED.tool_id, \
                 tool_name = EXCLUDED.tool_name, \
                 tool_name_canonical = EXCLUDED.tool_name_canonical, \
                 tool_input = EXCLUDED.tool_input, \
                 tool_output = EXCLUDED.tool_output, \
                 is_error = EXCLUDED.is_error, \
                 raw = EXCLUDED.raw",
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
        .bind(b.tool_output.as_ref())
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
    let codex_sessions: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT session_uuid \
         FROM events \
         WHERE agent = 'codex'",
    )
    .fetch_all(pool)
    .await?;
    let mut count = 0usize;
    for (session_uuid,) in codex_sessions {
        let rows: Vec<(Uuid, i64, String, serde_json::Value)> = sqlx::query_as(
            "SELECT session_uuid, byte_offset, agent, payload \
             FROM events \
             WHERE session_uuid = $1 \
             ORDER BY byte_offset",
        )
        .bind(session_uuid)
        .fetch_all(pool)
        .await?;
        let mut ctx = CodexSessionContext::new(session_uuid);
        for (row_session_uuid, byte_offset, agent, payload) in rows {
            let parsed =
                parse_canonical_event(&agent, &payload, row_session_uuid, byte_offset, Some(&ctx));
            let mut tx = pool.begin().await?;
            sqlx::query(
                "UPDATE events SET speaker = $3, content_kind = $4, \
                        event_uuid = $5, parent_event_uuid = $6, related_tool_use_id = $7, \
                        is_sidechain = $8, is_meta = $9, subtype = $10, search_text = $11 \
                 WHERE session_uuid = $1 AND byte_offset = $2",
            )
            .bind(row_session_uuid)
            .bind(byte_offset)
            .bind(parsed.speaker.as_str())
            .bind(parsed.content_kind.as_str())
            .bind(parsed.event_uuid.as_deref())
            .bind(parsed.parent_event_uuid.as_deref())
            .bind(parsed.related_tool_use_id.as_deref())
            .bind(parsed.is_sidechain)
            .bind(parsed.is_meta)
            .bind(parsed.subtype.as_deref())
            .bind(parsed.search_text())
            .execute(&mut *tx)
            .await?;
            insert_blocks(&mut tx, row_session_uuid, byte_offset, &parsed.blocks).await?;
            tx.commit().await?;
            if let Some(parent) = detect_codex_parent_session(&payload, row_session_uuid) {
                set_parent_session(pool, row_session_uuid, parent).await?;
            }
            update_codex_context(&mut ctx, &payload, row_session_uuid);
            count += 1;
        }
    }

    let rows: Vec<(Uuid, i64, String, serde_json::Value)> = sqlx::query_as(
        "SELECT e.session_uuid, e.byte_offset, e.agent, e.payload \
         FROM events e \
         WHERE e.agent <> 'codex' AND ( \
             e.search_text = '' OR \
             e.speaker IS NULL OR \
             e.content_kind IS NULL OR \
             e.event_uuid IS NULL OR \
             EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
                    AND b.kind = 'tool_result' \
                    AND b.tool_output IS NULL \
                    AND e.payload ? 'toolUseResult' \
             ) OR \
             EXISTS ( \
                 SELECT 1 FROM event_blocks b \
                  WHERE b.session_uuid = e.session_uuid \
                    AND b.byte_offset = e.byte_offset \
                    AND b.kind = 'tool_use' \
                    AND b.tool_name_canonical IN ('edit', 'multi_edit') \
                    AND b.tool_input IS NOT NULL \
                    AND NOT (b.tool_input ? 'file_edits') \
             ) OR \
             NOT EXISTS ( \
             SELECT 1 FROM event_blocks b \
              WHERE b.session_uuid = e.session_uuid \
                AND b.byte_offset = e.byte_offset \
         ))",
    )
    .fetch_all(pool)
    .await?;

    let legacy_count = rows.len();
    if legacy_count == 0 {
        return Ok(count);
    }

    for (session_uuid, byte_offset, agent, payload) in rows {
        let parsed = parse_canonical_event(&agent, &payload, session_uuid, byte_offset, None);
        let mut tx = pool.begin().await?;
        // Update the cached discriminator columns while we're here;
        // older rows may have the default 'claude-code' but stale or
        // missing canonical metadata.
        sqlx::query(
            "UPDATE events SET speaker = $3, content_kind = $4, \
                    event_uuid = $5, parent_event_uuid = $6, related_tool_use_id = $7, \
                    is_sidechain = $8, is_meta = $9, subtype = $10, search_text = $11 \
             WHERE session_uuid = $1 AND byte_offset = $2",
        )
        .bind(session_uuid)
        .bind(byte_offset)
        .bind(parsed.speaker.as_str())
        .bind(parsed.content_kind.as_str())
        .bind(parsed.event_uuid.as_deref())
        .bind(parsed.parent_event_uuid.as_deref())
        .bind(parsed.related_tool_use_id.as_deref())
        .bind(parsed.is_sidechain)
        .bind(parsed.is_meta)
        .bind(parsed.subtype.as_deref())
        .bind(parsed.search_text())
        .execute(&mut *tx)
        .await?;
        insert_blocks(&mut tx, session_uuid, byte_offset, &parsed.blocks).await?;
        tx.commit().await?;
    }
    Ok(count + legacy_count)
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
        assert_eq!(
            parse_session_uuid(&path, TranscriptSource::ClaudeCode),
            Some(uuid)
        );
    }
    #[test]
    fn parse_session_uuid_none_for_non_uuid_stem() {
        let path = PathBuf::from("/tmp/abc/not-a-uuid.jsonl");
        assert_eq!(
            parse_session_uuid(&path, TranscriptSource::ClaudeCode),
            None
        );
    }
    #[test]
    fn parse_project_hash_is_parent_dir() {
        let path = PathBuf::from("/tmp/my-project-hash/xxx.jsonl");
        assert_eq!(
            parse_project_hash(&path, TranscriptSource::ClaudeCode),
            Some("my-project-hash".into())
        );
    }
    #[test]
    fn parse_codex_session_uuid_from_rollout_filename() {
        let uuid = Uuid::new_v4();
        let path = PathBuf::from(format!(
            "/tmp/2026/04/19/rollout-2026-04-19T01-53-43-{uuid}.jsonl"
        ));
        assert_eq!(
            parse_session_uuid(&path, TranscriptSource::Codex),
            Some(uuid)
        );
    }
}
