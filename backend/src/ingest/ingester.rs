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

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::db::Pool;

use super::file_scan::{dirty_transcript_files, DirtyTranscriptFile};

mod canonical_backfill;
mod codex_lineage;
mod insert;
mod replay;

pub use replay::{replay_session_lines, ReplayLine, ReplayStats};

pub use canonical_backfill::backfill_canonical_blocks;
use canonical_backfill::set_parent_session;
use codex_lineage::load_codex_context;
use insert::{insert_event, InsertError};

/// Heartbeat interval for the "I'm alive, here's what I've done" log.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(60);

/// A tick whose dirty-file list is at least this long records itself as
/// a catch-up job, so a long drain (first-deploy import, downtime
/// backlog) is visible in the jobs panel rather than looking like a
/// stalled ingester.
const CATCHUP_JOB_THRESHOLD: usize = 10;

/// Lines one file may insert per tick, and the time it may spend doing so. A
/// busy transcript's backlog drains over several ticks instead of holding
/// every other file behind it; files with the least pending go first.
pub const ADMIT_LINES_PER_TICK: usize = 2000;
const ADMIT_TIME_PER_TICK: Duration = Duration::from_millis(250);

/// Sessions whose transcripts are ahead of their timeline, served one batch
/// at a time in arrival order so a long backlog cannot starve a quiet one.
#[derive(Debug, Default)]
struct ProjectionQueue {
    order: std::collections::VecDeque<Uuid>,
    queued: std::collections::HashSet<Uuid>,
}

impl ProjectionQueue {
    fn push(&mut self, session_uuid: Uuid) {
        if self.queued.insert(session_uuid) {
            self.order.push_back(session_uuid);
        }
    }

    fn pop(&mut self) -> Option<Uuid> {
        let session_uuid = self.order.pop_front()?;
        self.queued.remove(&session_uuid);
        Some(session_uuid)
    }
}

use super::tail::{next_line_boundary, MAX_READ_BYTES};

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
    // Catch-up job per transcript root, carried across ticks while a
    // backlog drains so the whole drain shows as one job row.
    catchup_jobs:
        tokio::sync::Mutex<std::collections::HashMap<&'static str, super::jobs::JobHandle>>,
    interrupted_stale_jobs: std::sync::atomic::AtomicBool,
    projections: std::sync::Mutex<ProjectionQueue>,
    projection_wake: tokio::sync::Notify,
    /// Sessions left behind by a previous process have been queued.
    projections_recovered: std::sync::atomic::AtomicBool,
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

        // Reading transcripts and projecting them run side by side: a slow
        // projection never holds up the next file read.
        tokio::join!(
            self.ingest_loop(&pool, &cfg),
            self.projection_loop(&pool, &cfg)
        );
    }

    async fn ingest_loop(&self, pool: &Pool, cfg: &IngesterConfig) {
        let mut last_heartbeat = Instant::now();

        loop {
            self.last_tick_started_at_unix
                .store(Utc::now().timestamp(), Ordering::Relaxed);
            let mut backlogged = false;
            match self.ingest(pool, cfg).await {
                Ok(summary) => {
                    backlogged = summary.backlogged;
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

            // A backlog keeps reading at once; its slices already bound how
            // long any other file waits.
            if backlogged {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(cfg.poll_interval).await;
            }
        }
    }

    /// Run one pass over every JSONL file, then project everything it
    /// queued. Exposed so tests and one-shot callers can drive the ingester
    /// synchronously; the long-running loop projects in parallel instead.
    pub async fn tick(&self, pool: &Pool, cfg: &IngesterConfig) -> anyhow::Result<TickSummary> {
        let summary = self.ingest(pool, cfg).await?;
        self.recover_projections(pool).await;
        while let Some(session_uuid) = self.next_projection() {
            self.project_one_batch(pool, session_uuid).await;
        }
        Ok(summary)
    }

    /// One pass over every JSONL file: insert new lines and queue their
    /// sessions for projection.
    async fn ingest(&self, pool: &Pool, cfg: &IngesterConfig) -> anyhow::Result<TickSummary> {
        // A previous process may have died mid-drain and left its
        // catch-up rows running; close them once before the first tick.
        if !self.interrupted_stale_jobs.swap(true, Ordering::Relaxed) {
            for source in [TranscriptSource::ClaudeCode, TranscriptSource::Codex] {
                if let Err(err) =
                    super::jobs::interrupt_running(pool, &catchup_job_name(source)).await
                {
                    tracing::warn!(%err, "stale catch-up job interruption failed");
                }
            }
        }
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

    fn queue_projection(&self, session_uuid: Uuid) {
        self.projections
            .lock()
            .expect("projection queue lock")
            .push(session_uuid);
        self.projection_wake.notify_one();
    }

    fn next_projection(&self) -> Option<Uuid> {
        self.projections
            .lock()
            .expect("projection queue lock")
            .pop()
    }

    /// Serve queued sessions one batch at a time, round robin, until the
    /// process exits.
    async fn projection_loop(&self, pool: &Pool, cfg: &IngesterConfig) {
        self.recover_projections(pool).await;
        loop {
            match self.next_projection() {
                Some(session_uuid) => self.project_one_batch(pool, session_uuid).await,
                None => {
                    let _ =
                        tokio::time::timeout(cfg.poll_interval, self.projection_wake.notified())
                            .await;
                }
            }
        }
    }

    /// One batch for the session; it goes to the back of the queue while it
    /// has more waiting.
    async fn project_one_batch(&self, pool: &Pool, session_uuid: Uuid) {
        match super::projection::project_batch(pool, session_uuid, super::projection::BATCH_EVENTS)
            .await
        {
            Ok(outcome) => {
                if outcome.more {
                    self.queue_projection(session_uuid);
                }
            }
            Err(err) => {
                // Left unqueued: its next insert, or the next process start,
                // retries from the committed cursor.
                tracing::warn!(
                    error = format!("{err:#}"),
                    session = %session_uuid,
                    "timeline projection batch failed",
                );
            }
        }
    }

    /// Queue sessions a previous process inserted events for but did not
    /// project. Sessions below the reducer version wait for the maintenance
    /// rebuild unless new events arrive for them first.
    async fn recover_projections(&self, pool: &Pool) {
        if self.projections_recovered.swap(true, Ordering::Relaxed) {
            return;
        }
        let owed: Result<Vec<(Uuid,)>, sqlx::Error> = sqlx::query_as(
            "SELECT i.session_uuid \
               FROM ingester_state i \
               JOIN claude_sessions cs ON cs.session_uuid = i.session_uuid AND cs.purged_at IS NULL \
               LEFT JOIN timeline_session_state s ON s.session_uuid = i.session_uuid \
              WHERE (s.session_uuid IS NULL OR s.projection_version = $1) \
                AND EXISTS (SELECT 1 FROM events e \
                             WHERE e.session_uuid = i.session_uuid \
                               AND e.byte_offset > COALESCE(s.projected_through, -1)) \
              ORDER BY i.updated_at DESC",
        )
        .bind(super::projection::REDUCER_VERSION)
        .fetch_all(pool)
        .await;
        match owed {
            Ok(owed) => {
                for (session_uuid,) in owed {
                    self.queue_projection(session_uuid);
                }
            }
            Err(err) => {
                self.projections_recovered.store(false, Ordering::Relaxed);
                tracing::warn!(%err, "owed timeline projection scan failed");
            }
        }
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
        let mut dirty_files = match dirty_transcript_files(pool, root, source).await {
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
        // A few new lines are read before a long backlog's slice.
        dirty_files.sort_by_key(|file| file.file_len - file.committed_offset);
        let job = self
            .catchup_job_for_tick(pool, source, dirty_files.len())
            .await;
        for file in &dirty_files {
            match process_file(pool, file, source).await {
                Ok(file_result) => {
                    summary.events_inserted += file_result.events_inserted;
                    summary.parse_errors += file_result.parse_errors;
                    summary.backlogged |= file_result.backlogged;
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
                    if file_result.events_inserted > 0 {
                        self.queue_projection(file.session_uuid);
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
            if let Some(job) = &job {
                job.advance(Some(&file.path.display().to_string())).await;
            }
        }
        if let Some(job) = job {
            // A short dirty list means the backlog has drained: the job
            // closes and steady-state ticks stop reporting. A long one
            // stashes the handle so the next tick keeps extending it.
            if dirty_files.len() < CATCHUP_JOB_THRESHOLD {
                job.complete().await;
            } else {
                self.catchup_jobs
                    .lock()
                    .await
                    .insert(source.agent_id(), job);
            }
        }
    }

    /// Resume the root's cross-tick catch-up job, or start one when this
    /// tick faces a backlog. Returns None during steady-state ticks.
    async fn catchup_job_for_tick(
        &self,
        pool: &Pool,
        source: TranscriptSource,
        dirty_count: usize,
    ) -> Option<super::jobs::JobHandle> {
        let existing = self.catchup_jobs.lock().await.remove(source.agent_id());
        match existing {
            Some(job) => {
                job.set_total(job.counted() + dirty_count as i64).await;
                Some(job)
            }
            None if dirty_count >= CATCHUP_JOB_THRESHOLD => super::jobs::start(
                pool,
                &catchup_job_name(source),
                &format!("{} transcript catch-up", source.agent_id()),
                "files",
                Some(dirty_count as i64),
            )
            .await
            .map_err(|err| tracing::warn!(%err, "catch-up job start failed"))
            .ok(),
            None => None,
        }
    }
}

fn catchup_job_name(source: TranscriptSource) -> String {
    format!("transcript_catchup_{}", source.agent_id())
}

fn unix_timestamp_from_atomic(value: &AtomicI64) -> Option<i64> {
    let raw = value.load(Ordering::Relaxed);
    (raw > 0).then_some(raw)
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TickSummary {
    pub events_inserted: u64,
    pub parse_errors: u64,
    /// Some file still had complete lines waiting when its admission ended.
    pub backlogged: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct FileResult {
    events_inserted: u64,
    parse_errors: u64,
    committed_offset: i64,
    /// Admission stopped with complete lines still waiting.
    backlogged: bool,
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

    // A claude subagent transcript: bind it to its parent session and
    // read the spawn's tool-use id from the sibling meta file so every
    // event links back to the parent's Agent pair.
    let mut subagent_tool_use_id: Option<String> = None;
    if let Some(parent) = file.subagent_parent {
        set_parent_session(pool, file.session_uuid, parent).await?;
        subagent_tool_use_id = read_subagent_tool_use_id(&file.path);
    }

    result.committed_offset = file.committed_offset;
    if replay::defer_if_purged(pool, file).await? {
        return Ok(result);
    }

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
    let pending = (file.file_len - file.committed_offset) as usize;
    let read_cap = pending.min(MAX_READ_BYTES);
    let mut buf = Vec::with_capacity(read_cap);
    transcript
        .by_ref()
        .take(read_cap as u64)
        .read_to_end(&mut buf)?;

    // Walk the buffer. For each newline-terminated line, insert an event
    // and advance `next_committed` past the newline.
    let mut line_start: usize = 0;
    let mut next_committed = file.committed_offset;
    let mut first_inserted_offset: Option<i64> = None;
    let mut codex_ctx = match source {
        TranscriptSource::Codex => Some(load_codex_context(pool, file.session_uuid).await?),
        TranscriptSource::ClaudeCode => None,
    };
    let mut admitted = 0usize;
    let admission_started = Instant::now();

    for (i, &b) in buf.iter().enumerate() {
        if b != b'\n' {
            continue;
        }
        if admission_closed(admitted, admission_started) {
            result.backlogged = true;
            break;
        }
        admitted += 1;
        let line = &buf[line_start..i];
        let byte_offset = file.committed_offset + line_start as i64;
        match insert_event(
            pool,
            file.session_uuid,
            source,
            byte_offset,
            line,
            codex_ctx.as_mut(),
            subagent_tool_use_id.as_deref(),
            None,
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
    if next_committed == file.committed_offset && buf.len() >= MAX_READ_BYTES {
        skip_oversized_line(pool, file, buf.len(), &mut result).await?;
        return Ok(result);
    }

    if next_committed != file.committed_offset {
        set_offset(pool, file.session_uuid, &file.path, next_committed).await?;
        result.committed_offset = next_committed;
    }
    rebuild_if_inserted_behind(pool, file.session_uuid, first_inserted_offset).await?;
    Ok(result)
}

/// A line longer than a whole tick's read is not a partial line we can wait
/// out, because waiting re-reads the cap every tick forever. Skip past it to
/// the next newline and resynchronise on a line boundary. Nothing is
/// committed for the skipped bytes, so the malformed region is dropped rather
/// than half-parsed.
async fn skip_oversized_line(
    pool: &Pool,
    file: &DirtyTranscriptFile,
    scanned: usize,
    result: &mut FileResult,
) -> anyhow::Result<()> {
    let scanned_to = file.committed_offset + scanned as i64;
    match next_line_boundary(&file.path, scanned_to)? {
        Some(resume) => {
            tracing::warn!(
                path = %file.path.display(),
                from = file.committed_offset,
                resume,
                "oversized transcript line; skipping to the next line boundary",
            );
            result.parse_errors += 1;
            set_offset(pool, file.session_uuid, &file.path, resume).await?;
            result.committed_offset = resume;
        }
        None => {
            // Still being written. The read is capped, so this costs a
            // bounded read per tick rather than unbounded growth.
            tracing::warn!(
                path = %file.path.display(),
                from = file.committed_offset,
                "transcript line exceeds the read cap and is not yet terminated",
            );
        }
    }
    Ok(())
}

/// A file's slice of this tick is spent; at least one line always goes in.
fn admission_closed(admitted: usize, started: Instant) -> bool {
    admitted == ADMIT_LINES_PER_TICK || (admitted > 0 && started.elapsed() >= ADMIT_TIME_PER_TICK)
}

/// A replaced transcript can insert lines at offsets the timeline has
/// already passed. The reducer only moves forward, so the session is
/// rebuilt from its canonical events instead.
async fn rebuild_if_inserted_behind(
    pool: &Pool,
    session_uuid: Uuid,
    first_inserted_offset: Option<i64>,
) -> anyhow::Result<()> {
    let Some(first_inserted_offset) = first_inserted_offset else {
        return Ok(());
    };
    let projected_through: Option<i64> = sqlx::query_scalar(
        "SELECT projected_through FROM timeline_session_state WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await?;
    if projected_through.is_some_and(|through| first_inserted_offset <= through) {
        tracing::warn!(
            session = %session_uuid,
            first_inserted_offset,
            "events inserted behind the timeline cursor; rebuilding the session",
        );
        super::projection::request_rebuild(pool, session_uuid).await?;
    }
    Ok(())
}

/// Best-effort read of the spawn linkage from the transcript's sibling
/// meta file (`agent-<id>.meta.json` beside `agent-<id>.jsonl`).
fn read_subagent_tool_use_id(path: &Path) -> Option<String> {
    let meta_path = path.with_extension("meta.json");
    let raw = std::fs::read_to_string(meta_path).ok()?;
    let meta: Value = serde_json::from_str(&raw).ok()?;
    meta.get("toolUseId")
        .and_then(Value::as_str)
        .map(|id| id.to_string())
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
