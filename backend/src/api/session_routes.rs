//! `/api/sessions*` handlers — spawning, updating, deleting, and history
//! reads. Ambient session listing is owned by `/api/app-state`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::routes::{repos_root, ApiError, ApiResult};
use crate::agent::AgentType;
use crate::ingest::{canonical, timeline};
use crate::pty::{self, AgentRuntimeMetadata, PtyMetadata, PtyWorkspaceMetadata, SpawnParams};
use crate::worktree::WorkspaceRecord;
use crate::AppState;

#[derive(Serialize)]
pub(super) struct SessionView {
    id: Uuid,
    repo: String,
    working_dir: String,
    workspace: Option<SessionWorkspaceView>,
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
    repo: String,
    working_dir: Option<String>,
    #[serde(default)]
    workspace_id: Option<Uuid>,
    #[serde(default)]
    workspace_mode: Option<String>,
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
    /// Optional first-class agent to launch immediately in the new PTY.
    #[serde(default)]
    launch_agent: Option<String>,
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

    // Resolve the repo root for fixture lookup. Workspace creation is
    // intentionally deferred until after all launch/resume validation
    // succeeds so bad requests don't leave stray worktrees behind.
    let repos_root = repos_root(&state)?;
    let workspace_mode = req.workspace_mode.as_deref().unwrap_or_else(|| {
        if req.launch_agent.is_some()
            || req.resume_session_uuid.is_some()
            || req.claude_resume_uuid.is_some()
        {
            "isolated"
        } else {
            "main"
        }
    });

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
    let launch_agent = req
        .launch_agent
        .as_deref()
        .map(parse_launch_agent)
        .transpose()?;
    if e2e_fixture.is_some() && (resume_session_uuid.is_some() || launch_agent.is_some()) {
        return Err(ApiError::BadRequest(
            "e2e_fixture cannot be combined with agent launch/resume".into(),
        ));
    }
    if resume_session_uuid.is_some() && launch_agent.is_some() {
        return Err(ApiError::BadRequest(
            "launch_agent cannot be combined with resume_session_uuid".into(),
        ));
    }
    let (shell, args, initial_agent_runtime_agent) =
        match (e2e_fixture, resume_session_uuid, resume_agent, launch_agent) {
            (Some(crate::e2e::MOCK_TERMINAL_FIXTURE), None, _, None) => {
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
                (path, Vec::new(), None)
            }
            (Some(fixture), None, _, None) => {
                return Err(ApiError::BadRequest(format!(
                    "unknown e2e fixture {fixture}",
                )));
            }
            (Some(_), Some(_), _, _) => unreachable!("fixture + resume handled above"),
            (Some(_), None, _, Some(_)) => unreachable!("fixture + launch handled above"),
            (None, Some(_), _, Some(_)) => unreachable!("resume + launch handled above"),
            (None, Some(uuid), Some("claude-code" | "claude"), None) => (
                PathBuf::from("/bin/bash"),
                vec![
                    "-c".to_string(),
                    agent_launch_shell_command(
                        AgentType::Claude,
                        &[
                            "--dangerously-skip-permissions".to_string(),
                            "--resume".to_string(),
                            uuid.to_string(),
                        ],
                        true,
                    ),
                ],
                Some("claude".to_string()),
            ),
            (None, Some(uuid), Some("codex"), None) => (
                PathBuf::from("/bin/bash"),
                vec![
                    "-c".to_string(),
                    agent_launch_shell_command(
                        AgentType::Codex,
                        &["--yolo".to_string(), "resume".to_string(), uuid.to_string()],
                        true,
                    ),
                ],
                Some("codex".to_string()),
            ),
            (None, Some(_), Some(agent), None) => {
                return Err(ApiError::BadRequest(format!(
                    "resume is not implemented for agent {agent}",
                )));
            }
            (None, Some(_), None, None) => {
                return Err(ApiError::BadRequest(
                    "resume_agent is required when resume_session_uuid is set".into(),
                ));
            }
            (None, None, _, Some(agent)) => (
                PathBuf::from("/bin/bash"),
                vec!["-c".to_string(), default_agent_launch_command(agent, true)],
                Some(agent.as_str().to_string()),
            ),
            (None, None, _, None) => (pty::default_shell(), Vec::new(), None),
        };

    let workspace_record = resolve_session_workspace(&state, &req, workspace_mode).await?;
    let workspace_root = workspace_record.path.clone();
    let working_dir = match (&req.working_dir, workspace_mode) {
        (Some(p), "main") => PathBuf::from(p),
        (Some(_), "isolated") | (Some(_), "worktree") => {
            return Err(ApiError::BadRequest(
                "working_dir is only supported with workspace_mode=main".into(),
            ));
        }
        (Some(_), _) => unreachable!("workspace mode validated in resolve_session_workspace"),
        (None, _) => workspace_root,
    };
    if !working_dir.exists() {
        return Err(ApiError::BadRequest(format!(
            "working dir does not exist: {}",
            working_dir.display()
        )));
    }

    let params = SpawnParams {
        repo: req.repo.clone(),
        working_dir,
        workspace: Some(pty_workspace_metadata(&workspace_record)),
        shell,
        args,
        cols: req.cols.unwrap_or(120),
        rows: req.rows.unwrap_or(32),
        initial_agent_runtime_agent,
    };
    let meta = state.pty.spawn(params).await?;
    state
        .workspace_state
        .bind_created_session(workspace_record.id, meta.id)
        .await
        .map_err(ApiError::Internal)?;
    Ok((StatusCode::CREATED, Json(SessionView::from(meta))))
}

async fn resolve_session_workspace(
    state: &AppState,
    req: &CreateSessionReq,
    workspace_mode: &str,
) -> ApiResult<WorkspaceRecord> {
    validate_workspace_request(req, workspace_mode)?;
    if let Some(id) = req.workspace_id {
        let workspace = state
            .workspace_state
            .load_workspace(id)
            .await
            .map_err(|_| ApiError::NotFound)?;
        if workspace.repo_name != req.repo {
            return Err(ApiError::BadRequest(
                "workspace_id does not belong to requested repo".into(),
            ));
        }
        if workspace.state != "active" {
            return Err(ApiError::BadRequest("workspace is not active".into()));
        }
        return Ok(workspace);
    }

    match workspace_mode {
        "main" => {
            let repo_path = repo_path_for_session(state, &req.repo)?;
            state
                .workspace_state
                .ensure_main_workspace(&req.repo, &repo_path)
                .await
                .map_err(ApiError::Internal)
        }
        "isolated" | "worktree" => state
            .workspace_state
            .create_worktree_workspace(&req.repo)
            .await
            .map_err(|err| ApiError::BadRequest(err.to_string())),
        _ => Err(ApiError::BadRequest(
            "workspace_mode must be one of: main, isolated".into(),
        )),
    }
}

fn validate_workspace_request(req: &CreateSessionReq, workspace_mode: &str) -> ApiResult<()> {
    if req.workspace_id.is_some() && req.workspace_mode.is_some() {
        return Err(ApiError::BadRequest(
            "workspace_id cannot be combined with workspace_mode".into(),
        ));
    }
    if req.workspace_id.is_some() && req.working_dir.is_some() {
        return Err(ApiError::BadRequest(
            "workspace_id cannot be combined with working_dir".into(),
        ));
    }
    match workspace_mode {
        "main" | "isolated" | "worktree" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "workspace_mode must be one of: main, isolated".into(),
        )),
    }
}

fn repo_path_for_session(state: &AppState, repo: &str) -> ApiResult<PathBuf> {
    if repo.is_empty() || repo.contains('/') || repo.starts_with('.') {
        return Err(ApiError::BadRequest("invalid repo name".into()));
    }
    let path = state.repos_root.join(repo);
    if !path.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "repo does not exist: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn pty_workspace_metadata(record: &WorkspaceRecord) -> PtyWorkspaceMetadata {
    PtyWorkspaceMetadata {
        id: record.id,
        repo_name: record.repo_name.clone(),
        kind: record.kind.clone(),
        path: record.path.clone(),
        branch_name: record.branch_name.clone(),
        base_ref: record.base_ref.clone(),
        base_sha: record.base_sha.clone(),
        merge_target: record.merge_target.clone(),
    }
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

    state.pty.mark_agent_starting(id, agent.as_str()).await?;
    let command = format!("{}\r", default_agent_launch_command(agent, false));
    state.pty.send_input(id, command.into_bytes()).await?;
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
    state
        .pty
        .send_input(id, prompt_input_bytes(&req.text))
        .await?;
    Ok(StatusCode::ACCEPTED)
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
    shell_quote_str(&path.to_string_lossy())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn parse_launch_agent(raw: &str) -> ApiResult<AgentType> {
    AgentType::parse(raw.trim())
        .map_err(|_| ApiError::BadRequest("agent must be one of: claude, codex".to_string()))
}

fn default_agent_launch_command(agent: AgentType, append_exec_bash: bool) -> String {
    match agent {
        AgentType::Claude => agent_launch_shell_command(
            agent,
            &["--dangerously-skip-permissions".to_string()],
            append_exec_bash,
        ),
        AgentType::Codex => {
            agent_launch_shell_command(agent, &["--yolo".to_string()], append_exec_bash)
        }
    }
}

fn agent_launch_shell_command(
    agent: AgentType,
    agent_args: &[String],
    append_exec_bash: bool,
) -> String {
    let mut parts = vec![
        shell_quote(&crate::agent::binary_path()),
        "agent-launcher".to_string(),
        "--type".to_string(),
        agent.as_str().to_string(),
        "--mode".to_string(),
        "real".to_string(),
        "--".to_string(),
    ];
    parts.extend(agent_args.iter().map(|arg| shell_quote_str(arg)));
    let mut command = parts.join(" ");
    if append_exec_bash {
        command.push_str(" ; exec bash");
    }
    command
}

fn prompt_input_bytes(text: &str) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.contains('\n') {
        format!("\x1b[200~{normalized}\x1b[201~\r").into_bytes()
    } else {
        format!("{normalized}\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_input_submits_single_line_with_enter() {
        assert_eq!(prompt_input_bytes("hello"), b"hello\r");
    }

    #[test]
    fn prompt_input_uses_bracketed_paste_for_multiline_text() {
        assert_eq!(
            prompt_input_bytes("hello\r\nworld"),
            b"\x1b[200~hello\nworld\x1b[201~\r",
        );
    }
}
