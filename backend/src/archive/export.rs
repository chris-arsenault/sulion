//! Exporting a session from `events.payload` to the object store.
//!
//! One object per agent session, one JSON line per stored event in byte
//! order. Each line is an envelope around the original payload:
//!
//! ```text
//! {"o": <byte_offset>, "t": "<timestamp>", "k": "<kind>", "r": <related_tool_use_id>, "p": {…}}
//! ```
//!
//! The offset is what makes a restore idempotent against a transcript file
//! that still exists: replayed rows land on the same `(session, offset)` key
//! the file would produce, so a later append to that file inserts only the
//! new lines. The timestamp and tool-use link are the two columns the
//! ingester cannot always re-derive from the payload alone.

use std::collections::BTreeMap;
use std::io::Write;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::db::Pool;

use super::store::ObjectStore;

pub const OBJECT_PREFIX: &str = "sessions";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveLine {
    pub o: i64,
    pub t: DateTime<Utc>,
    pub k: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
    pub p: Value,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EligibleSession {
    pub session_uuid: Uuid,
    pub agent: String,
    pub event_count: i64,
    pub first_event_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    pub archive_events: Option<i64>,
}

/// Sessions whose export is missing or stale: idle past the window, not the
/// current session of a live PTY, with events to export, and either never
/// exported or grown since.
pub async fn eligible_sessions(
    pool: &Pool,
    min_idle_days: i64,
) -> anyhow::Result<Vec<EligibleSession>> {
    let rows: Vec<EligibleSession> = sqlx::query_as(
        "SELECT cs.session_uuid, cs.agent, \
                counts.event_count, counts.first_event_at, \
                cs.archived_at, cs.archive_events \
           FROM claude_sessions cs \
           JOIN LATERAL ( \
                SELECT COUNT(*)::BIGINT AS event_count, MIN(e.timestamp) AS first_event_at, \
                       MAX(e.timestamp) AS last_event_at \
                  FROM events e WHERE e.session_uuid = cs.session_uuid \
           ) counts ON TRUE \
          WHERE cs.purged_at IS NULL \
            AND counts.event_count > 0 \
            AND counts.last_event_at < NOW() - make_interval(days => $1::INT) \
            AND (cs.archived_at IS NULL OR cs.archive_events IS DISTINCT FROM counts.event_count) \
            AND NOT EXISTS ( \
                SELECT 1 FROM pty_sessions ps \
                 WHERE ps.current_session_uuid = cs.session_uuid AND ps.state = 'live' \
            ) \
          ORDER BY counts.first_event_at ASC",
    )
    .bind(min_idle_days as i32)
    .fetch_all(pool)
    .await
    .context("select sessions eligible for export")?;
    Ok(rows)
}

pub fn object_key(agent: &str, first_event_at: DateTime<Utc>, session_uuid: Uuid) -> String {
    format!(
        "{OBJECT_PREFIX}/{agent}/{}/{session_uuid}.jsonl.zst",
        first_event_at.format("%Y/%m")
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportOutcome {
    pub session_uuid: Uuid,
    pub key: String,
    pub events: i64,
    pub bytes: i64,
    pub compressed_bytes: i64,
    pub sha256: String,
}

/// Streams the session's events into a compressed temp file, uploads it,
/// verifies the object by `HEAD`, and records the archive columns. The
/// temp file never holds more than one session.
pub async fn export_session(
    pool: &Pool,
    store: &ObjectStore,
    session: &EligibleSession,
) -> anyhow::Result<ExportOutcome> {
    let key = object_key(&session.agent, session.first_event_at, session.session_uuid);
    let temp = tempfile::NamedTempFile::new().context("create export temp file")?;
    let (bytes, sha256, events) =
        write_session_lines(pool, session.session_uuid, temp.path()).await?;
    if events == 0 {
        anyhow::bail!("session {} has no events to export", session.session_uuid);
    }
    let compressed_bytes = std::fs::metadata(temp.path())?.len() as i64;

    let mut metadata = BTreeMap::new();
    metadata.insert("session-uuid".to_string(), session.session_uuid.to_string());
    metadata.insert("agent".to_string(), session.agent.clone());
    metadata.insert("events".to_string(), events.to_string());
    metadata.insert("sha256".to_string(), sha256.clone());
    metadata.insert("bytes".to_string(), bytes.to_string());
    store
        .put_file(&key, temp.path(), &metadata)
        .await
        .with_context(|| format!("upload {key}"))?;

    let head = store
        .head(&key)
        .await?
        .ok_or_else(|| anyhow!("{key} is absent after upload"))?;
    if head.content_length != compressed_bytes {
        anyhow::bail!(
            "{key}: stored {} bytes, uploaded {compressed_bytes}",
            head.content_length
        );
    }
    if head.metadata.get("sha256").map(String::as_str) != Some(sha256.as_str()) {
        anyhow::bail!("{key}: stored sha256 does not match the export");
    }

    sqlx::query(
        "UPDATE claude_sessions \
            SET archived_at = NOW(), archive_key = $2, archive_sha256 = $3, \
                archive_bytes = $4, archive_events = $5 \
          WHERE session_uuid = $1",
    )
    .bind(session.session_uuid)
    .bind(&key)
    .bind(&sha256)
    .bind(bytes)
    .bind(events)
    .execute(pool)
    .await
    .context("record archive columns")?;

    Ok(ExportOutcome {
        session_uuid: session.session_uuid,
        key,
        events,
        bytes,
        compressed_bytes,
        sha256,
    })
}

/// Writes the envelope lines, zstd-compressed, to `path`. Returns the
/// uncompressed byte count, its sha256, and the number of lines.
async fn write_session_lines(
    pool: &Pool,
    session_uuid: Uuid,
    path: &std::path::Path,
) -> anyhow::Result<(i64, String, i64)> {
    let file = std::fs::File::create(path)?;
    let mut encoder = zstd::stream::Encoder::new(file, 3).context("zstd encoder")?;
    let mut hasher = digest::Context::new(&digest::SHA256);
    let mut bytes: i64 = 0;
    let mut events: i64 = 0;

    let mut rows = sqlx::query(
        "SELECT byte_offset, timestamp, kind, related_tool_use_id, payload \
           FROM events WHERE session_uuid = $1 AND payload IS NOT NULL \
          ORDER BY byte_offset ASC",
    )
    .bind(session_uuid)
    .fetch(pool);
    while let Some(row) = rows.try_next().await? {
        let line = ArchiveLine {
            o: row.try_get("byte_offset")?,
            t: row.try_get("timestamp")?,
            k: row.try_get("kind")?,
            r: row.try_get("related_tool_use_id").ok().flatten(),
            p: row.try_get("payload")?,
        };
        let mut encoded = serde_json::to_vec(&line)?;
        encoded.push(b'\n');
        hasher.update(&encoded);
        bytes += encoded.len() as i64;
        events += 1;
        encoder.write_all(&encoded)?;
    }
    encoder.finish()?.sync_all()?;
    let sha256 = hex(hasher.finish().as_ref());
    Ok((bytes, sha256, events))
}

/// Reads an archive object back into envelope lines, verifying the sha256
/// of the decompressed bytes against what the session row recorded.
pub async fn read_session_lines(
    store: &ObjectStore,
    key: &str,
    expected_sha256: &str,
) -> anyhow::Result<Vec<ArchiveLine>> {
    let temp = tempfile::NamedTempFile::new().context("create restore temp file")?;
    store.get_to_file(key, temp.path()).await?;
    let compressed = std::fs::File::open(temp.path())?;
    let mut decoder = zstd::stream::Decoder::new(compressed).context("zstd decoder")?;
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut raw)?;
    let actual = hex(digest::digest(&digest::SHA256, &raw).as_ref());
    if actual != expected_sha256 {
        anyhow::bail!("{key}: sha256 {actual} does not match recorded {expected_sha256}");
    }
    let mut lines = Vec::new();
    for (index, line) in raw.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            continue;
        }
        let parsed: ArchiveLine = serde_json::from_slice(line)
            .with_context(|| format!("{key}: line {} is not an archive envelope", index + 1))?;
        lines.push(parsed);
    }
    Ok(lines)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VerifyOutcome {
    pub deep: bool,
    pub sessions_archived: usize,
    pub ok: usize,
    pub missing: usize,
    pub mismatched: usize,
    /// Sessions whose events changed since export; a later cycle re-exports
    /// them, so they are reported but not counted as failures.
    pub stale: usize,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ArchivedSession {
    session_uuid: Uuid,
    archive_key: String,
    archive_sha256: String,
    archive_bytes: i64,
    archive_events: i64,
    purged_at: Option<DateTime<Utc>>,
    event_count: i64,
}

/// Checks every archived session's object against what the session row
/// recorded. Shallow: `HEAD` for existence and the stored hash and counts.
/// Deep: also downloads and decompresses each object and re-hashes the
/// bytes, which is the check to run once before opening the purge gate.
pub async fn verify_archives(
    pool: &Pool,
    store: &ObjectStore,
    deep: bool,
    job: Option<&crate::ingest::jobs::JobHandle>,
) -> anyhow::Result<VerifyOutcome> {
    let sessions: Vec<ArchivedSession> = sqlx::query_as(
        "SELECT cs.session_uuid, cs.archive_key, cs.archive_sha256, cs.archive_bytes, \
                cs.archive_events, cs.purged_at, \
                (SELECT COUNT(*)::BIGINT FROM events e WHERE e.session_uuid = cs.session_uuid) AS event_count \
           FROM claude_sessions cs \
          WHERE cs.archived_at IS NOT NULL AND cs.archive_key IS NOT NULL \
          ORDER BY cs.archive_key ASC",
    )
    .fetch_all(pool)
    .await
    .context("select archived sessions")?;
    let mut outcome = VerifyOutcome {
        deep,
        sessions_archived: sessions.len(),
        ..VerifyOutcome::default()
    };
    if let Some(job) = job {
        job.set_total(sessions.len() as i64).await;
    }
    for session in &sessions {
        if let Some(job) = job {
            job.advance(Some(&session.archive_key)).await;
        }
        if session.purged_at.is_none() && session.event_count != session.archive_events {
            outcome.stale += 1;
        }
        let head = match store.head(&session.archive_key).await {
            Ok(Some(head)) => head,
            Ok(None) => {
                outcome.missing += 1;
                outcome.problems.push(format!(
                    "{}: object {} is missing",
                    session.session_uuid, session.archive_key
                ));
                continue;
            }
            Err(err) => {
                outcome.missing += 1;
                outcome.problems.push(format!(
                    "{}: HEAD {} failed: {err:#}",
                    session.session_uuid, session.archive_key
                ));
                continue;
            }
        };
        let mut mismatch = Vec::new();
        if head.metadata.get("sha256").map(String::as_str) != Some(session.archive_sha256.as_str())
        {
            mismatch.push("stored sha256 differs from the session row".to_string());
        }
        if head
            .metadata
            .get("events")
            .and_then(|v| v.parse::<i64>().ok())
            != Some(session.archive_events)
        {
            mismatch.push("stored event count differs from the session row".to_string());
        }
        if head
            .metadata
            .get("bytes")
            .and_then(|v| v.parse::<i64>().ok())
            != Some(session.archive_bytes)
        {
            mismatch.push("stored byte count differs from the session row".to_string());
        }
        if deep && mismatch.is_empty() {
            match read_session_lines(store, &session.archive_key, &session.archive_sha256).await {
                Ok(lines) if lines.len() as i64 == session.archive_events => {}
                Ok(lines) => mismatch.push(format!(
                    "object holds {} lines, session row says {}",
                    lines.len(),
                    session.archive_events
                )),
                Err(err) => mismatch.push(format!("content check failed: {err:#}")),
            }
        }
        if mismatch.is_empty() {
            outcome.ok += 1;
        } else {
            outcome.mismatched += 1;
            outcome.problems.push(format!(
                "{} ({}): {}",
                session.session_uuid,
                session.archive_key,
                mismatch.join("; ")
            ));
        }
    }
    Ok(outcome)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
