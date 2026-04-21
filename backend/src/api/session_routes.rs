//! `/api/sessions*` handlers — listing, spawning, updating, and
//! history reads. Extracted from `routes.rs` to keep the router shell
//! thin; the route wiring itself still lives in `routes::router()`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::future_prompt_routes;
use super::routes::{repos_root, ApiError, ApiResult};
use crate::ingest::{canonical, timeline};
use crate::pty::{self, PtyMetadata, SpawnParams};
use crate::AppState;

#[derive(Serialize)]
pub(super) struct SessionView {
    id: Uuid,
    repo: String,
    working_dir: String,
    state: &'static str,
    created_at: chrono::DateTime<chrono::Utc>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
    current_session_uuid: Option<Uuid>,
    current_session_agent: Option<String>,
    /// MAX(event.timestamp) for this session's current transcript session.
    /// Null means no events ingested yet. Used by the frontend's
    /// unread-dot indicator.
    last_event_at: Option<chrono::DateTime<chrono::Utc>>,
    /// User-facing label; overrides the uuid prefix in the sidebar.
    label: Option<String>,
    /// Pinned sessions float to the top of their repo group.
    pinned: bool,
    /// Palette-constrained colour tag name.
    color: Option<String>,
    /// Number of `pending` entries in the session's future-prompts
    /// directory — powers the sidebar badge. Always 0 for sessions
    /// without a correlated transcript session_uuid.
    future_prompts_pending_count: u32,
}

/// Allowed palette names for session colour tags. The backend rejects
/// anything outside this set so invalid strings don't sneak into the
/// UI and produce unstyled chips.
const COLOR_PALETTE: &[&str] = &[
    "amber", "emerald", "sky", "rose", "violet", "slate", "teal", "fuchsia",
];

impl From<PtyMetadata> for SessionView {
    fn from(m: PtyMetadata) -> Self {
        let state = match m.state {
            pty::PtyState::Live => "live",
            pty::PtyState::Dead => "dead",
            pty::PtyState::Deleted => "deleted",
            pty::PtyState::Orphaned => "orphaned",
        };
        Self {
            id: m.id,
            repo: m.repo,
            working_dir: m.working_dir.to_string_lossy().into_owned(),
            state,
            created_at: m.created_at,
            ended_at: m.ended_at,
            exit_code: m.exit_code,
            current_session_uuid: m.current_session_uuid,
            current_session_agent: m.current_session_agent,
            last_event_at: m.last_event_at,
            label: m.label,
            pinned: m.pinned,
            color: m.color,
            future_prompts_pending_count: 0,
        }
    }
}

#[derive(Serialize)]
pub(super) struct ListSessionsResponse {
    sessions: Vec<SessionView>,
}

pub(super) async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ListSessionsResponse>> {
    let metas = state.pty.list().await?;
    let mut sessions: Vec<SessionView> = metas.into_iter().map(SessionView::from).collect();

    // Fan out future-prompt counts over the filesystem. Sessions
    // without a correlated transcript session_uuid can't have entries
    // anyway, so their count stays 0 without a filesystem probe.
    let root = future_prompt_routes::future_prompts_root(&state);
    let count_futures = sessions.iter().map(|s| {
        let root = root.clone();
        let uuid = s.current_session_uuid;
        async move {
            match uuid {
                Some(u) => crate::future_prompts::count_pending(&root, u)
                    .await
                    .unwrap_or(0),
                None => 0,
            }
        }
    });
    let counts = futures::future::join_all(count_futures).await;
    for (sv, c) in sessions.iter_mut().zip(counts) {
        sv.future_prompts_pending_count = u32::try_from(c).unwrap_or(u32::MAX);
    }

    Ok(Json(ListSessionsResponse { sessions }))
}

#[derive(Deserialize)]
pub(super) struct CreateSessionReq {
    repo: String,
    working_dir: Option<String>,
    #[serde(default)]
    cols: Option<u16>,
    #[serde(default)]
    rows: Option<u16>,
    /// If set, the shell boots straight into the agent-specific resume
    /// command and falls back to an interactive shell after.
    #[serde(default)]
    resume_session_uuid: Option<Uuid>,
    #[serde(default)]
    resume_agent: Option<String>,
    /// Backward-compatible alias for older frontend builds.
    #[serde(default)]
    claude_resume_uuid: Option<Uuid>,
    /// Test-only scripted fixture. Rejected unless the backend was
    /// started with `SULION_ENABLE_E2E_FIXTURES=1`.
    #[serde(default)]
    e2e_fixture: Option<String>,
}

pub(super) async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionReq>,
) -> ApiResult<(StatusCode, Json<SessionView>)> {
    if req.repo.is_empty() {
        return Err(ApiError::BadRequest("repo must not be empty".into()));
    }

    // Resolve working directory. If absent, use <repos_root>/<repo>.
    let repos_root = repos_root(&state)?;
    let working_dir = match &req.working_dir {
        Some(p) => PathBuf::from(p),
        None => repos_root.join(&req.repo),
    };
    if !working_dir.exists() {
        return Err(ApiError::BadRequest(format!(
            "working dir does not exist: {}",
            working_dir.display()
        )));
    }

    // When resuming a prior agent session, boot bash directly into the
    // agent-specific resume command and fall back to an interactive
    // shell after. The uuid is `Uuid`-typed so no shell injection is
    // possible.
    let e2e_fixture = req
        .e2e_fixture
        .as_deref()
        .map(str::trim)
        .filter(|fixture| !fixture.is_empty());
    let resume_session_uuid = req.resume_session_uuid.or(req.claude_resume_uuid);
    let resume_agent = req
        .resume_agent
        .as_deref()
        .or_else(|| resume_session_uuid.map(|_| "claude-code"));
    if e2e_fixture.is_some() && resume_session_uuid.is_some() {
        return Err(ApiError::BadRequest(
            "e2e_fixture cannot be combined with resume_session_uuid".into(),
        ));
    }
    let (shell, args) = match (e2e_fixture, resume_session_uuid, resume_agent) {
        (Some(crate::e2e::MOCK_TERMINAL_FIXTURE), None, _) => {
            if !crate::e2e::fixtures_enabled() {
                return Err(ApiError::BadRequest(
                    "e2e fixtures are disabled on this backend".into(),
                ));
            }
            let path = crate::e2e::mock_terminal_script_path(&repos_root);
            if !path.is_file() {
                return Err(ApiError::Internal(anyhow::anyhow!(
                    "mock terminal fixture missing: {}",
                    path.display()
                )));
            }
            (path, Vec::new())
        }
        (Some(fixture), None, _) => {
            return Err(ApiError::BadRequest(format!(
                "unknown e2e fixture {fixture}",
            )));
        }
        (Some(_), Some(_), _) => unreachable!("fixture + resume handled above"),
        (None, Some(uuid), Some("claude-code")) => (
            PathBuf::from("/bin/bash"),
            vec![
                "-c".to_string(),
                format!(
                    "{} agent-launcher --type claude --mode real -- --dangerously-skip-permissions --resume {uuid} ; exec bash",
                    shell_quote(&crate::agent::binary_path())
                ),
            ],
        ),
        (None, Some(uuid), Some("codex")) => (
            PathBuf::from("/bin/bash"),
            vec![
                "-c".to_string(),
                format!(
                    "{} agent-launcher --type codex --mode real -- --yolo resume {uuid} ; exec bash",
                    shell_quote(&crate::agent::binary_path())
                ),
            ],
        ),
        (None, Some(_), Some(agent)) => {
            return Err(ApiError::BadRequest(format!(
                "resume is not implemented for agent {agent}",
            )));
        }
        (None, Some(_), None) => {
            return Err(ApiError::BadRequest(
                "resume_agent is required when resume_session_uuid is set".into(),
            ));
        }
        (None, None, _) => (pty::default_shell(), Vec::new()),
    };

    let params = SpawnParams {
        repo: req.repo.clone(),
        working_dir,
        shell,
        args,
        cols: req.cols.unwrap_or(120),
        rows: req.rows.unwrap_or(32),
    };
    let meta = state.pty.spawn(params).await?;
    Ok((StatusCode::CREATED, Json(SessionView::from(meta))))
}

pub(super) async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    state.pty.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct PatchSessionReq {
    /// Set the label. Empty string clears. Null/absent = no change.
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    pinned: Option<bool>,
    /// One of COLOR_PALETTE names, or empty string to clear. Null =
    /// no change.
    #[serde(default)]
    color: Option<String>,
}

pub(super) async fn patch_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchSessionReq>,
) -> ApiResult<StatusCode> {
    if let Some(name) = &req.color {
        if !name.is_empty() && !COLOR_PALETTE.contains(&name.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "color must be one of: {}",
                COLOR_PALETTE.join(", "),
            )));
        }
    }
    if let Some(label) = &req.label {
        if label.len() > 100 {
            return Err(ApiError::BadRequest(
                "label must be 100 characters or fewer".into(),
            ));
        }
    }
    state
        .pty
        .update_metadata(id, req.label.map(Some), req.pinned, req.color.map(Some))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct HistoryQuery {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    session: Option<Uuid>,
    #[serde(default)]
    claude_session: Option<Uuid>,
}

#[derive(Serialize)]
pub(super) struct EventView {
    byte_offset: i64,
    timestamp: chrono::DateTime<chrono::Utc>,
    kind: String,
    agent: String,
    speaker: Option<String>,
    content_kind: Option<String>,
    event_uuid: Option<String>,
    parent_event_uuid: Option<String>,
    related_tool_use_id: Option<String>,
    is_sidechain: bool,
    is_meta: bool,
    subtype: Option<String>,
    /// Canonical content blocks, agent-agnostic. Empty for unparsable
    /// events or those still waiting on the startup backfill.
    blocks: Vec<canonical::Block>,
}

#[derive(Serialize)]
pub(super) struct HistoryResponse {
    session_uuid: Option<Uuid>,
    session_agent: Option<String>,
    events: Vec<EventView>,
    next_after: Option<i64>,
}

pub(super) async fn session_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<HistoryResponse>> {
    let resolved =
        timeline::resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        timeline::SessionLookup::Resolved(resolved) => resolved,
        timeline::SessionLookup::NoSession => {
            return Ok(Json(HistoryResponse {
                session_uuid: None,
                session_agent: None,
                events: Vec::new(),
                next_after: None,
            }));
        }
        timeline::SessionLookup::MissingPty => return Err(ApiError::NotFound),
    };
    let events = timeline::load_session_events(
        &state.pool,
        resolved.session_uuid,
        &timeline::SessionEventFilter {
            after: q.after,
            limit: Some(q.limit.unwrap_or(5000)),
            kind: q.kind.clone(),
        },
    )
    .await?;

    let next_after = events.last().map(|event| event.byte_offset);

    let events = events
        .into_iter()
        .map(|event| EventView {
            byte_offset: event.byte_offset,
            timestamp: event.timestamp,
            kind: event.kind,
            agent: event.agent,
            speaker: event.speaker,
            content_kind: event.content_kind,
            event_uuid: event.event_uuid,
            parent_event_uuid: event.parent_event_uuid,
            related_tool_use_id: event.related_tool_use_id,
            is_sidechain: event.is_sidechain,
            is_meta: event.is_meta,
            subtype: event.subtype,
            blocks: event.blocks,
        })
        .collect();

    Ok(Json(HistoryResponse {
        session_uuid: Some(resolved.session_uuid),
        session_agent: resolved.session_agent,
        events,
        next_after,
    }))
}

pub(super) async fn drop_session_ws(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    if !crate::e2e::fixtures_enabled() {
        return Err(ApiError::NotFound);
    }
    if state.ws_test_hooks.drop_live_ws(id).await {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
