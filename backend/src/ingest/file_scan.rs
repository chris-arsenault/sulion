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
}

#[derive(Debug, Clone)]
struct TranscriptFile {
    path: PathBuf,
    session_uuid: Uuid,
    project_hash: Option<String>,
    file_len: i64,
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
    let Some(session_uuid) = parse_session_uuid(path, source) else {
        tracing::debug!(
            agent = source.agent_id(),
            path = %path.display(),
            "skipping: filename does not encode a supported session uuid",
        );
        return None;
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
        project_hash: parse_project_hash(path, source),
        file_len,
    })
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
