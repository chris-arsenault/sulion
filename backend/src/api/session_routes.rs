//! `/api/sessions*` handlers — spawning, updating, deleting, and history
//! reads. Ambient session listing is owned by `/api/app-state`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::node_proxy;
use super::routes::{ApiError, ApiResult};
use super::session_launch::{
    protocol_working_dir, requested_workspace_mode, resolve_protocol_launch,
    validate_workspace_request,
};
use crate::agent::AgentType;
use crate::ingest::{
    load_session_events, resolve_session_target, CanonicalBlock, SessionEventFilter, SessionLookup,
};
use crate::node_protocol::NodeRequestKind;
use crate::node_runtime::{
    AgentRequest, ResourceRequest, SessionCreateRequest, SessionInputRequest,
    SessionMetaRepoRequest, SessionRepoRequest,
};
use crate::pty::{self, AgentRuntimeMetadata, PtyMetadata};
use crate::AppState;

#[derive(Serialize)]
pub(super) struct SessionView {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace: Option<SessionWorkspaceView>,
    meta_repo: Option<SessionMetaRepoView>,
    repositories: Vec<SessionRepositoryView>,
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
    agent_runtime: AgentRuntimeView,
    agent_metadata: Option<AgentSessionMetadataView>,
    /// Number of `pending` entries in the session's future-prompts
    /// directory — powers the sidebar badge. Always 0 for sessions
    /// without a correlated transcript session_uuid.
    future_prompts_pending_count: u32,
}

#[derive(Serialize)]
pub(super) struct SessionWorkspaceView {
    id: Uuid,
    repo_name: String,
    kind: String,
    path: String,
    branch_name: Option<String>,
    base_ref: Option<String>,
    base_sha: Option<String>,
    merge_target: Option<String>,
}

#[derive(Serialize)]
pub(super) struct SessionMetaRepoView {
    id: Uuid,
    name: String,
}

#[derive(Serialize)]
pub(super) struct SessionRepositoryView {
    repo_name: String,
    workspace_id: Option<Uuid>,
    role: String,
    position: i32,
}

#[derive(Serialize)]
pub(super) struct AgentRuntimeView {
    agent: Option<String>,
    state: String,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    ended_at: Option<chrono::DateTime<chrono::Utc>>,
    exit_code: Option<i32>,
}

impl From<AgentRuntimeMetadata> for AgentRuntimeView {
    fn from(runtime: AgentRuntimeMetadata) -> Self {
        Self {
            agent: runtime.agent,
            state: runtime.state,
            started_at: runtime.started_at,
            ended_at: runtime.ended_at,
            exit_code: runtime.exit_code,
        }
    }
}

#[derive(Serialize)]
pub(super) struct AgentSessionMetadataView {
    agent: String,
    model: Option<String>,
    model_provider: Option<String>,
    reasoning_effort: Option<String>,
    cli_version: Option<String>,
    cwd: Option<String>,
    model_context_window: Option<i64>,
    updated_at: chrono::DateTime<chrono::Utc>,
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
            workspace: m.workspace.map(|workspace| SessionWorkspaceView {
                id: workspace.id,
                repo_name: workspace.repo_name,
                kind: workspace.kind,
                path: workspace.path.to_string_lossy().into_owned(),
                branch_name: workspace.branch_name,
                base_ref: workspace.base_ref,
                base_sha: workspace.base_sha,
                merge_target: workspace.merge_target,
            }),
            meta_repo: m.meta_repo.map(|group| SessionMetaRepoView {
                id: group.id,
                name: group.name,
            }),
            repositories: m
                .repositories
                .into_iter()
                .map(|repository| SessionRepositoryView {
                    repo_name: repository.repo_name,
                    workspace_id: repository.workspace.map(|workspace| workspace.id),
                    role: repository.role,
                    position: repository.position,
                })
                .collect(),
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
            agent_runtime: AgentRuntimeView::from(m.agent_runtime),
            agent_metadata: None,
            future_prompts_pending_count: 0,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct CreateSessionReq {
    #[serde(default)]
    pub(super) repo: Option<String>,
    #[serde(default)]
    pub(super) meta_repo_id: Option<Uuid>,
    #[serde(default)]
    pub(super) scope_source_session_id: Option<Uuid>,
    pub(super) working_dir: Option<String>,
    #[serde(default)]
    pub(super) workspace_id: Option<Uuid>,
    #[serde(default)]
    pub(super) workspace_mode: Option<String>,
    #[serde(default)]
    pub(super) cols: Option<u16>,
    #[serde(default)]
    pub(super) rows: Option<u16>,
    /// If set, the shell boots straight into the agent-specific resume
    /// command and falls back to an interactive shell after.
    #[serde(default)]
    pub(super) resume_session_uuid: Option<Uuid>,
    #[serde(default)]
    pub(super) resume_agent: Option<String>,
    /// Alias for `resume_session_uuid`. No shipped frontend sends it — the
    /// resume hook sends the pair above — but a browser tab left open holds its
    /// JS indefinitely, so the reader stays until stale tabs are no longer a
    /// concern. Removing it also drops the agent inference in
    /// `resolve_protocol_launch` that exists only for callers of this field.
    #[serde(default)]
    pub(super) claude_resume_uuid: Option<Uuid>,
    /// Optional first-class agent to launch immediately in the new PTY.
    #[serde(default)]
    pub(super) launch_agent: Option<String>,
    /// Test-only scripted fixture. Rejected unless the backend was
    /// started with `SULION_ENABLE_E2E_FIXTURES=1`.
    #[serde(default)]
    pub(super) e2e_fixture: Option<String>,
}

pub(super) async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSessionReq>,
) -> ApiResult<(StatusCode, Json<SessionView>)> {
    create_session_on_node(&state, req).await
}

struct ResolvedSessionScope {
    meta_repo: Option<SessionMetaRepoRequest>,
    members: Vec<SessionRepoRequest>,
    workspace_mode: String,
    collection: bool,
}

#[derive(sqlx::FromRow)]
struct SourceSessionRepoRow {
    repo_name: String,
    workspace_id: Option<Uuid>,
    role: String,
    position: i32,
    workspace_kind: Option<String>,
}

async fn create_session_on_node(
    state: &AppState,
    req: CreateSessionReq,
) -> ApiResult<(StatusCode, Json<SessionView>)> {
    let launch = resolve_protocol_launch(&req)?;
    let scope = resolve_session_scope(state, &req).await?;
    let primary = scope
        .members
        .first()
        .ok_or_else(|| ApiError::BadRequest("session requires a repository".into()))?;
    let working_dir = req
        .working_dir
        .as_deref()
        .map(|path| protocol_working_dir(&state.repos_root, &primary.repo, path))
        .transpose()?;
    let node_id = node_proxy::default_node(state).await?;
    if scope.collection
        && !state
            .node_control
            .supports_capability(node_id, crate::node_protocol::MULTI_REPO_SESSION_CAPABILITY)
            .await
    {
        return Err(ApiError::Unavailable(
            "the development node must finish updating before it can launch a meta-repository session"
                .into(),
        ));
    }
    let session_id = Uuid::new_v4();
    let request = SessionCreateRequest {
        session_id,
        allocated_workspace_id: primary.allocated_workspace_id,
        existing_workspace_id: primary.existing_workspace_id,
        repo: primary.repo.clone(),
        working_dir,
        workspace_mode: scope.workspace_mode,
        cols: req.cols.unwrap_or(120),
        rows: req.rows.unwrap_or(32),
        launch,
        meta_repo: scope.meta_repo,
        additional_repos: scope.members.into_iter().skip(1).collect(),
    };
    let result = node_proxy::request(
        state,
        node_id,
        NodeRequestKind::SessionCreate,
        serde_json::to_value(request).map_err(anyhow::Error::from)?,
    )
    .await?;
    let metadata: PtyMetadata = serde_json::from_value(result).map_err(anyhow::Error::from)?;
    Ok((StatusCode::CREATED, Json(SessionView::from(metadata))))
}

async fn resolve_session_scope(
    state: &AppState,
    req: &CreateSessionReq,
) -> ApiResult<ResolvedSessionScope> {
    let selector_count = usize::from(req.repo.is_some())
        + usize::from(req.meta_repo_id.is_some())
        + usize::from(req.scope_source_session_id.is_some());
    if selector_count != 1 {
        return Err(ApiError::BadRequest(
            "supply exactly one of repo, meta_repo_id, or scope_source_session_id".into(),
        ));
    }

    if let Some(source_id) = req.scope_source_session_id {
        return resolve_source_session_scope(state, req, source_id).await;
    }

    let workspace_mode = requested_workspace_mode(req).to_string();
    validate_workspace_request(req, &workspace_mode)?;
    if let Some(meta_repo_id) = req.meta_repo_id {
        return resolve_meta_repo_scope(state, req, meta_repo_id, workspace_mode).await;
    }

    let repo = req
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|repo| !repo.is_empty())
        .ok_or_else(|| ApiError::BadRequest("repo must not be empty".into()))?;
    Ok(ResolvedSessionScope {
        meta_repo: None,
        members: vec![SessionRepoRequest {
            repo: repo.to_string(),
            allocated_workspace_id: Uuid::new_v4(),
            existing_workspace_id: req.workspace_id,
            position: 0,
        }],
        workspace_mode,
        collection: false,
    })
}

async fn resolve_source_session_scope(
    state: &AppState,
    req: &CreateSessionReq,
    source_id: Uuid,
) -> ApiResult<ResolvedSessionScope> {
    if req.workspace_id.is_some() {
        return Err(ApiError::BadRequest(
            "workspace_id cannot be combined with scope_source_session_id".into(),
        ));
    }
    let source: Option<(Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT meta_repo_id, meta_repo_name FROM pty_sessions \
          WHERE id = $1 AND state <> 'deleted'",
    )
    .bind(source_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((meta_repo_id, meta_repo_name)) = source else {
        return Err(ApiError::NotFound);
    };
    let rows = sqlx::query_as::<_, SourceSessionRepoRow>(
        "SELECT psr.repo_name, psr.workspace_id, psr.role, psr.position, \
                ws.kind AS workspace_kind \
           FROM pty_session_repos psr \
           LEFT JOIN workspaces ws ON ws.id = psr.workspace_id \
          WHERE psr.pty_session_id = $1 \
          ORDER BY psr.position",
    )
    .bind(source_id)
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() || rows.first().is_none_or(|row| row.role != "primary") {
        return Err(ApiError::BadRequest(
            "source session has no reusable repository scope".into(),
        ));
    }
    let workspace_mode = source_workspace_mode(&rows)?;
    if req
        .workspace_mode
        .as_deref()
        .is_some_and(|requested| requested != workspace_mode)
    {
        return Err(ApiError::BadRequest(
            "workspace_mode must match the source session".into(),
        ));
    }
    if req.working_dir.is_some() && workspace_mode != "main" {
        return Err(ApiError::BadRequest(
            "working_dir is only supported with workspace_mode=main".into(),
        ));
    }
    let members = rows
        .into_iter()
        .map(|row| {
            let workspace_id = row.workspace_id.ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "source session repository {} has no workspace",
                    row.repo_name
                ))
            })?;
            Ok(SessionRepoRequest {
                repo: row.repo_name,
                allocated_workspace_id: Uuid::new_v4(),
                existing_workspace_id: Some(workspace_id),
                position: row.position,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let meta_repo = meta_repo_id.map(|id| SessionMetaRepoRequest {
        id,
        name: meta_repo_name.unwrap_or_else(|| "Deleted meta-repository".into()),
    });
    let collection = meta_repo.is_some() || members.len() > 1;
    Ok(ResolvedSessionScope {
        meta_repo,
        members,
        workspace_mode: workspace_mode.into(),
        collection,
    })
}

async fn resolve_meta_repo_scope(
    state: &AppState,
    req: &CreateSessionReq,
    meta_repo_id: Uuid,
    workspace_mode: String,
) -> ApiResult<ResolvedSessionScope> {
    if req.workspace_id.is_some() {
        return Err(ApiError::BadRequest(
            "workspace_id cannot be combined with meta_repo_id".into(),
        ));
    }
    let group = crate::meta_repos::get(&state.pool, meta_repo_id).await?;
    if group.members.is_empty() {
        return Err(ApiError::BadRequest(
            "meta-repository needs at least one member before launching a session".into(),
        ));
    }
    let missing = group
        .members
        .iter()
        .filter(|member| !member.exists)
        .map(|member| member.repo_name.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "meta-repository member is missing: {}",
            missing.join(", ")
        )));
    }
    let primary = group
        .primary_repo_name
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("meta-repository has no primary repository".into()))?;
    let mut ordered = group.members;
    ordered.sort_by_key(|member| (member.repo_name != primary, member.position));
    let members = ordered
        .into_iter()
        .enumerate()
        .map(|(position, member)| SessionRepoRequest {
            repo: member.repo_name,
            allocated_workspace_id: Uuid::new_v4(),
            existing_workspace_id: None,
            position: position as i32,
        })
        .collect();
    Ok(ResolvedSessionScope {
        meta_repo: Some(SessionMetaRepoRequest {
            id: group.id,
            name: group.name,
        }),
        members,
        workspace_mode,
        collection: true,
    })
}

fn source_workspace_mode(rows: &[SourceSessionRepoRow]) -> ApiResult<&'static str> {
    let mut mode = None;
    for row in rows {
        let current = match row.workspace_kind.as_deref() {
            Some("main") => "main",
            Some("worktree") => "isolated",
            _ => {
                return Err(ApiError::BadRequest(format!(
                    "source session repository {} has no active workspace",
                    row.repo_name
                )))
            }
        };
        if mode.is_some_and(|mode| mode != current) {
            return Err(ApiError::BadRequest(
                "source session mixes workspace modes".into(),
            ));
        }
        mode = Some(current);
    }
    mode.ok_or_else(|| ApiError::BadRequest("source session has no workspaces".into()))
}

#[derive(Deserialize)]
pub(super) struct StartAgentReq {
    agent: String,
}

pub(super) async fn start_session_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<StartAgentReq>,
) -> ApiResult<StatusCode> {
    let agent = parse_launch_agent(&req.agent)?;
    let meta = pty::read_meta(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if meta.state != pty::PtyState::Live {
        return Err(ApiError::BadRequest("PTY session is not live".into()));
    }
    if matches!(meta.agent_runtime.state.as_str(), "starting" | "running") {
        return Err(ApiError::BadRequest(format!(
            "{} is already {}",
            meta.agent_runtime.agent.as_deref().unwrap_or("agent"),
            meta.agent_runtime.state,
        )));
    }

    let node_id = node_proxy::session_node(&state, id).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::SessionAgentStart,
        serde_json::to_value(AgentRequest {
            session_id: id,
            agent: agent.as_str().into(),
        })
        .map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn interrupt_session_agent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let meta = pty::read_meta(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if meta.state != pty::PtyState::Live {
        return Err(ApiError::BadRequest("PTY session is not live".into()));
    }
    if !matches!(meta.agent_runtime.state.as_str(), "starting" | "running") {
        return Err(ApiError::BadRequest("agent is not running".into()));
    }
    let node_id = node_proxy::session_node(&state, id).await?;
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::SessionAgentInterrupt,
        serde_json::to_value(ResourceRequest { id }).map_err(anyhow::Error::from)?,
    )
    .await?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
pub(super) struct PromptReq {
    text: String,
}

pub(super) async fn send_session_prompt(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PromptReq>,
) -> ApiResult<StatusCode> {
    if req.text.trim().is_empty() {
        return Err(ApiError::BadRequest("prompt text must not be empty".into()));
    }
    let meta = pty::read_meta(&state.pool, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if meta.state != pty::PtyState::Live {
        return Err(ApiError::BadRequest("PTY session is not live".into()));
    }
    if meta.agent_runtime.state != "running" {
        return Err(ApiError::BadRequest("agent is not running".into()));
    }
    let node_id = node_proxy::session_node(&state, id).await?;
    for (index, chunk) in prompt_input_chunks(&req.text).into_iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(
                AGENT_PROMPT_SUBMIT_DELAY_MS,
            ))
            .await;
        }
        node_proxy::request(
            &state,
            node_id,
            NodeRequestKind::SessionInput,
            serde_json::to_value(SessionInputRequest::from_bytes(id, &chunk))
                .map_err(anyhow::Error::from)?,
        )
        .await?;
    }
    crate::activity::set(
        &state.pool,
        id,
        crate::activity::ActivityState::Working,
        Some(&req.text),
        None,
        "user",
        "explicit",
    )
    .await
    .map_err(ApiError::Internal)?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let (node_id, session_state): (Option<Uuid>, String) =
        sqlx::query_as("SELECT node_id, state FROM pty_sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::NotFound)?;
    // Forward only when the owning node is connected and can reap the
    // process. Husks — orphaned or ended sessions, including rows from
    // the legacy local runtime or a node identity that no longer exists
    // — have no process anywhere, and refusing to delete them strands
    // them in the sidebar forever (the resume flow deletes the husk it
    // replaces).
    match node_id {
        Some(node_id) if state.node_control.is_connected(node_id).await => {
            node_proxy::request(
                &state,
                node_id,
                NodeRequestKind::SessionDelete,
                serde_json::to_value(ResourceRequest { id }).map_err(anyhow::Error::from)?,
            )
            .await?;
            return Ok(StatusCode::NO_CONTENT);
        }
        _ if session_state == "live" => {
            return Err(ApiError::Unavailable(
                "session's development node is not connected".into(),
            ));
        }
        _ => {}
    }
    state.pty.delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves a session's shell onto the current toolset. Forward-only: unlike
/// delete there is no husk fallback, because an upgrade without a live
/// process is meaningless.
pub(super) async fn upgrade_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let (node_id, session_state): (Option<Uuid>, String) =
        sqlx::query_as("SELECT node_id, state FROM pty_sessions WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(ApiError::NotFound)?;
    if session_state != "live" {
        return Err(ApiError::BadRequest(
            "only live sessions can be upgraded".into(),
        ));
    }
    let Some(node_id) = node_id else {
        return Err(ApiError::Unavailable(
            "session has no development node".into(),
        ));
    };
    if !state.node_control.is_connected(node_id).await {
        return Err(ApiError::Unavailable(
            "session's development node is not connected".into(),
        ));
    }
    node_proxy::request(
        &state,
        node_id,
        NodeRequestKind::SessionUpgrade,
        serde_json::to_value(ResourceRequest { id }).map_err(anyhow::Error::from)?,
    )
    .await?;
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
    blocks: Vec<CanonicalBlock>,
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
    let resolved = resolve_session_target(&state.pool, id, q.session.or(q.claude_session)).await?;

    let resolved = match resolved {
        SessionLookup::Resolved(resolved) => resolved,
        SessionLookup::NoSession => {
            return Ok(Json(HistoryResponse {
                session_uuid: None,
                session_agent: None,
                events: Vec::new(),
                next_after: None,
            }));
        }
        SessionLookup::MissingPty => return Err(ApiError::NotFound),
    };
    let events = load_session_events(
        &state.pool,
        resolved.session_uuid,
        &SessionEventFilter {
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

pub(super) fn parse_launch_agent(raw: &str) -> ApiResult<AgentType> {
    AgentType::parse(raw.trim())
        .map_err(|_| ApiError::BadRequest("agent must be one of: claude, codex".to_string()))
}

const AGENT_PROMPT_SUBMIT_DELAY_MS: u64 = 50;

fn prompt_input_chunks(text: &str) -> Vec<Vec<u8>> {
    let normalized = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .replace('\n', "\r")
        .to_string();
    let prompt = format!("\x1b[200~{normalized}\x1b[201~").into_bytes();
    vec![prompt, agent_submit_input()]
}

fn agent_submit_input() -> Vec<u8> {
    b"\r".to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_input_pastes_single_line_before_enter() {
        assert_eq!(
            prompt_input_chunks("hello"),
            vec![b"\x1b[200~hello\x1b[201~".to_vec(), b"\r".to_vec()]
        );
    }

    #[test]
    fn prompt_input_strips_trailing_textarea_newline_before_enter() {
        assert_eq!(
            prompt_input_chunks("hello\n"),
            vec![b"\x1b[200~hello\x1b[201~".to_vec(), b"\r".to_vec()],
        );
        assert_eq!(
            prompt_input_chunks("hello\r\n"),
            vec![b"\x1b[200~hello\x1b[201~".to_vec(), b"\r".to_vec()],
        );
    }

    #[test]
    fn prompt_input_normalizes_multiline_text_for_terminal_paste() {
        assert_eq!(
            prompt_input_chunks("hello\r\nworld\n"),
            vec![b"\x1b[200~hello\rworld\x1b[201~".to_vec(), b"\r".to_vec()],
        );
    }
}
