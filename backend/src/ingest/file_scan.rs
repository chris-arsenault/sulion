use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::db::Pool;

use super::ingester::{parse_project_hash, parse_session_uuid, TranscriptSource};

#[derive(Debug, Clone)]
pub(super) struct DirtyTranscriptFile {
    pub path: PathBuf,
    pub session_uuid: Uuid,
    pub project_hash: Option<String>,
    pub committed_offset: i64,
    pub file_len: i64,
    /// Set when this is a claude `subagents/agent-*.jsonl` transcript:
    /// the parent session it spawned from. `session_uuid` is then a
    /// synthetic id derived from the parent and the agent filename, so
    /// the (session_uuid, byte_offset) idempotency key stays sound.
    pub subagent_parent: Option<Uuid>,
}

#[derive(Debug, Clone)]
struct TranscriptFile {
    path: PathBuf,
    session_uuid: Uuid,
    project_hash: Option<String>,
    file_len: i64,
    subagent_parent: Option<Uuid>,
}

pub(super) async fn dirty_transcript_files(
    pool: &Pool,
    root: &Path,
    source: TranscriptSource,
) -> anyhow::Result<Vec<DirtyTranscriptFile>> {
    let files = discover_transcript_files(root, source);
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let session_ids = files
        .iter()
        .map(|file| file.session_uuid)
        .collect::<Vec<_>>();
    let committed_offsets = load_committed_offsets(pool, &session_ids).await?;

    Ok(files
        .into_iter()
        .filter_map(|file| {
            let committed_offset = committed_offsets
                .get(&file.session_uuid)
                .copied()
                .unwrap_or(0);
            (file.file_len != committed_offset).then_some(DirtyTranscriptFile {
                path: file.path,
                session_uuid: file.session_uuid,
                project_hash: file.project_hash,
                committed_offset,
                file_len: file.file_len,
                subagent_parent: file.subagent_parent,
            })
        })
        .collect())
}

fn discover_transcript_files(root: &Path, source: TranscriptSource) -> Vec<TranscriptFile> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter_map(|entry| discovered_transcript_file(entry.path(), source))
        .collect()
}

fn discovered_transcript_file(path: &Path, source: TranscriptSource) -> Option<TranscriptFile> {
    let (session_uuid, subagent_parent, project_hash) = match parse_session_uuid(path, source) {
        Some(session_uuid) => (session_uuid, None, parse_project_hash(path, source)),
        None => {
            let Some(link) = parse_claude_subagent_file(path, source) else {
                tracing::debug!(
                    agent = source.agent_id(),
                    path = %path.display(),
                    "skipping: filename does not encode a supported session uuid",
                );
                return None;
            };
            link
        }
    };
    let file_len = match std::fs::metadata(path) {
        Ok(md) => md.len() as i64,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "stat failed");
            return None;
        }
    };
    Some(TranscriptFile {
        path: path.to_path_buf(),
        session_uuid,
        project_hash,
        file_len,
        subagent_parent,
    })
}

/// Claude background-agent transcripts live under the parent session's
/// directory: `<project>/<parent-uuid>/subagents/agent-<id>.jsonl`. Each
/// file becomes its own child session with a deterministic synthetic
/// uuid so per-session offsets and the event idempotency key both hold.
fn parse_claude_subagent_file(
    path: &Path,
    source: TranscriptSource,
) -> Option<(Uuid, Option<Uuid>, Option<String>)> {
    if source != TranscriptSource::ClaudeCode {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if !stem.starts_with("agent-") {
        return None;
    }
    let subagents_dir = path.parent()?;
    if subagents_dir.file_name()?.to_str()? != "subagents" {
        return None;
    }
    let parent_dir = subagents_dir.parent()?;
    let parent_uuid = Uuid::parse_str(parent_dir.file_name()?.to_str()?).ok()?;
    let session_uuid = Uuid::new_v5(&parent_uuid, stem.as_bytes());
    let project_hash = parent_dir
        .parent()
        .and_then(|dir| dir.file_name())
        .and_then(|name| name.to_str())
        .map(|name| name.to_string());
    Some((session_uuid, Some(parent_uuid), project_hash))
}

async fn load_committed_offsets(
    pool: &Pool,
    session_ids: &[Uuid],
) -> anyhow::Result<HashMap<Uuid, i64>> {
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        "SELECT session_uuid, file_path, last_committed_byte_offset \
           FROM ingester_state \
          WHERE session_uuid = ANY($1)",
    )
    .bind(session_ids)
    .fetch_all(pool)
    .await
    .context("load ingester state bitmap")?;

    Ok(rows
        .into_iter()
        .map(|(session_uuid, _file_path, offset)| (session_uuid, offset))
        .collect())
}
