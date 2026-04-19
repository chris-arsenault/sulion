//! Search surface. Three scopes:
//!
//! - `timeline`  → Postgres ILIKE over canonical `events.search_text`
//!   for one transcript session's history.
//! - `repo`      → spawn `rg` streaming file-content matches under the
//!   repo root, plus `timeline` unioned across every session in that
//!   repo.
//! - `workspace` → same as `repo` but across every repo.
//!
//! File search runs as a subprocess; output is a stream of one JSON
//! object per line to the HTTP client (newline-delimited JSON).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

use crate::routes::ApiError;
use crate::AppState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    /// "timeline" | "repo" | "workspace"
    pub scope: String,
    pub repo: Option<String>,
    pub session: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum SearchHit {
    #[serde(rename = "file")]
    File {
        repo: String,
        path: String,
        line: u32,
        preview: String,
    },
    #[serde(rename = "event")]
    Event {
        session_id: String,
        session_uuid: String,
        session_agent: String,
        byte_offset: i64,
        kind: String,
        timestamp: String,
        preview: String,
    },
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "error")]
    Error { message: String },
}

pub async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<SearchQuery>,
) -> Result<axum::response::Response, ApiError> {
    let query = q.q.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::BadRequest("q required".into()));
    }

    let state = state.clone();
    let stream = async_stream::stream! {
        let mut emitted_any = false;

        match q.scope.as_str() {
            "timeline" => {
                for hit in search_timeline(&state, q.session.as_deref(), &query).await.unwrap_or_default() {
                    emitted_any = true;
                    yield Ok::<_, std::io::Error>(encode(&hit));
                }
            }
            "repo" | "workspace" => {
                let repos = if q.scope == "repo" {
                    q.repo.as_deref().map(|r| vec![r.to_string()]).unwrap_or_default()
                } else {
                    // workspace: list every subdirectory under repos_root
                    list_repo_names(&state.repos_root).await.unwrap_or_default()
                };

                // Timeline hits first across every session in scope, then
                // file hits streamed from rg.
                for hit in search_timeline(&state, q.session.as_deref(), &query).await.unwrap_or_default() {
                    emitted_any = true;
                    yield Ok(encode(&hit));
                }

                for repo_name in repos {
                    let repo_path = state.repos_root.join(&repo_name);
                    if !repo_path.is_dir() {
                        continue;
                    }
                    let mut rx = spawn_ripgrep(&repo_name, &repo_path, &query);
                    while let Some(hit) = rx.next().await {
                        emitted_any = true;
                        yield Ok(encode(&hit));
                    }
                }
            }
            _ => {
                yield Ok(encode(&SearchHit::Error {
                    message: format!("unknown scope: {}", q.scope),
                }));
            }
        }

        let _ = emitted_any;
        yield Ok(encode(&SearchHit::Done));
    };

    let body = Body::from_stream(stream);
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response())
}

fn encode(hit: &SearchHit) -> bytes::Bytes {
    let mut buf = serde_json::to_vec(hit).unwrap_or_default();
    buf.push(b'\n');
    bytes::Bytes::from(buf)
}

async fn list_repo_names(root: &Path) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    if !root.exists() {
        return Ok(names);
    }
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(e) = entries.next_entry().await? {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if let Ok(meta) = e.metadata().await {
            if meta.is_dir() {
                names.push(name);
            }
        }
    }
    Ok(names)
}

/// Stream ripgrep JSON output for a repo, converting each match record
/// into a `SearchHit::File`. Binds to a reasonable set of flags that
/// respect `.gitignore` and cap output.
fn spawn_ripgrep(
    repo_name: &str,
    repo_path: &Path,
    query: &str,
) -> futures::stream::BoxStream<'static, SearchHit> {
    let repo_name = repo_name.to_string();
    let cwd = repo_path.to_path_buf();
    let q = query.to_string();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SearchHit>();

    tokio::spawn(async move {
        let mut cmd = Command::new("rg");
        cmd.arg("--json")
            .arg("--max-count=50")
            .arg("--max-filesize=1M")
            .arg("--no-heading")
            .arg("--smart-case")
            .arg(&q)
            .current_dir(&cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                let _ = tx.send(SearchHit::Error {
                    message: format!("rg spawn failed: {err}"),
                });
                return;
            }
        };
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => return,
        };
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(hit) = parse_rg_line(&line, &repo_name) {
                if tx.send(hit).is_err() {
                    break;
                }
            }
        }
        let _ = child.wait().await;
    });

    futures::stream::unfold(
        rx,
        |mut rx| async move { rx.recv().await.map(|hit| (hit, rx)) },
    )
    .boxed()
}

fn parse_rg_line(line: &str, repo_name: &str) -> Option<SearchHit> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "match" {
        return None;
    }
    let data = v.get("data")?;
    let path = data.get("path")?.get("text")?.as_str()?.to_string();
    let line_no = data.get("line_number")?.as_u64()? as u32;
    let preview = data
        .get("lines")
        .and_then(|l| l.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim_end_matches('\n')
        .to_string();
    Some(SearchHit::File {
        repo: repo_name.to_string(),
        path,
        line: line_no,
        preview,
    })
}

/// Timeline full-text: scans canonical `events.search_text` with ILIKE
/// for a quick-and-simple match. A GIN/tsvector index would scale
/// better, but we're at <10k events per session today — the simpler
/// path buys enough runway.
async fn search_timeline(
    state: &AppState,
    session_id: Option<&str>,
    query: &str,
) -> anyhow::Result<Vec<SearchHit>> {
    let pattern = format!("%{}%", query);
    let session_uuid = match session_id {
        Some(id) => {
            let uuid = uuid::Uuid::parse_str(id)?;
            // pty_sessions.id -> current transcript session uuid
            let row: Option<(Option<uuid::Uuid>,)> =
                sqlx::query_as("SELECT current_session_uuid FROM pty_sessions WHERE id = $1")
                    .bind(uuid)
                    .fetch_optional(&state.pool)
                    .await?;
            match row {
                Some((Some(u),)) => Some((uuid, u)),
                _ => return Ok(Vec::new()),
            }
        }
        None => None,
    };

    let rows: Vec<(
        PathBuf, // dummy so Rust doesn't infer too hard; replaced below
    )>;
    let _ = rows; // suppress unused

    let out = if let Some((pty_id, session_uuid)) = session_uuid {
        let pty_id_str = pty_id.to_string();
        let session_uuid_str = session_uuid.to_string();
        sqlx::query_as::<_, (i64, chrono::DateTime<chrono::Utc>, String, String, String)>(
            "SELECT e.byte_offset, e.timestamp, e.kind, e.search_text, s.agent \
             FROM events e \
             JOIN claude_sessions s ON s.session_uuid = e.session_uuid \
             WHERE e.session_uuid = $1 AND e.search_text ILIKE $2 \
             ORDER BY byte_offset DESC \
             LIMIT 100",
        )
        .bind(session_uuid)
        .bind(&pattern)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(
            |(offset, ts, kind, search_text, session_agent)| SearchHit::Event {
                session_id: pty_id_str.clone(),
                session_uuid: session_uuid_str.clone(),
                session_agent,
                byte_offset: offset,
                kind,
                timestamp: ts.to_rfc3339(),
                preview: snippet(&search_text, query),
            },
        )
        .collect()
    } else {
        Vec::new()
    };

    Ok(out)
}

fn snippet(search_text: &str, query: &str) -> String {
    // Extract a ±80-char window around the first match. Keeps preview
    // small without yanking the whole event back to the client.
    let s = search_text;
    let lower = s.to_lowercase();
    let q = query.to_lowercase();
    if let Some(idx) = lower.find(&q) {
        let start = idx.saturating_sub(80);
        let end = (idx + q.len() + 80).min(s.len());
        let mut snip = String::new();
        if start > 0 {
            snip.push('…');
        }
        snip.push_str(&s[start..end]);
        if end < s.len() {
            snip.push('…');
        }
        snip
    } else {
        s.chars().take(160).collect()
    }
}
