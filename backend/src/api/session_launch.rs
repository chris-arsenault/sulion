//! Resolving what a session should launch.
//!
//! Which shell, which working directory, which workspace, and which agent a
//! new session starts with. Separated from the routes because it is pure
//! decision-making over the request and the repo layout, with no HTTP or
//! database concerns of its own.

use std::path::{Path as StdPath, PathBuf};

use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use super::session_routes::{
    agent_launch_shell_command, default_agent_launch_command, parse_launch_agent, CreateSessionReq,
};
use crate::agent::AgentType;
use crate::node_runtime::SessionLaunch as NodeSessionLaunch;
use crate::pty::PtyWorkspaceMetadata;
use crate::worktree::WorkspaceRecord;
use crate::AppState;

pub(super) fn resolve_protocol_launch(req: &CreateSessionReq) -> ApiResult<NodeSessionLaunch> {
    let fixture = req
        .e2e_fixture
        .as_deref()
        .map(str::trim)
        .filter(|fixture| !fixture.is_empty());
    let resume_session_uuid = req.resume_session_uuid.or(req.claude_resume_uuid);
    let resume_agent = req
        .resume_agent
        .as_deref()
        .or_else(|| resume_session_uuid.map(|_| "claude-code"));
    if fixture.is_some() && (resume_session_uuid.is_some() || req.launch_agent.is_some()) {
        return Err(ApiError::BadRequest(
            "e2e_fixture cannot be combined with agent launch/resume".into(),
        ));
    }
    if resume_session_uuid.is_some() && req.launch_agent.is_some() {
        return Err(ApiError::BadRequest(
            "launch_agent cannot be combined with resume_session_uuid".into(),
        ));
    }
    if let Some(fixture) = fixture {
        if fixture != crate::e2e::MOCK_TERMINAL_FIXTURE {
            return Err(ApiError::BadRequest(format!(
                "unknown e2e fixture {fixture}"
            )));
        }
        if !crate::e2e::fixtures_enabled() {
            return Err(ApiError::BadRequest(
                "e2e fixtures are disabled on this control plane".into(),
            ));
        }
        return Ok(NodeSessionLaunch::MockTerminal);
    }
    if let Some(session_id) = resume_session_uuid {
        let agent = resume_agent.ok_or_else(|| {
            ApiError::BadRequest("resume_agent is required when resume_session_uuid is set".into())
        })?;
        let agent = parse_launch_agent(agent)?;
        return Ok(NodeSessionLaunch::Agent {
            agent: agent.as_str().into(),
            resume_session_uuid: Some(session_id),
        });
    }
    if let Some(agent) = req.launch_agent.as_deref() {
        let agent = parse_launch_agent(agent)?;
        return Ok(NodeSessionLaunch::Agent {
            agent: agent.as_str().into(),
            resume_session_uuid: None,
        });
    }
    Ok(NodeSessionLaunch::Shell)
}

pub(super) fn protocol_working_dir(repo: &str, value: &str) -> ApiResult<String> {
    let path = StdPath::new(value);
    if !path.is_absolute() {
        return Ok(value.to_string());
    }
    let canonical_root = PathBuf::from("/home/sulion/repos").join(repo);
    path.strip_prefix(&canonical_root)
        .map(|relative| relative.to_string_lossy().into_owned())
        .map_err(|_| {
            ApiError::BadRequest(
                "absolute working_dir must be inside the canonical repo path".into(),
            )
        })
}

pub(super) struct SessionLaunch {
    pub(super) shell: PathBuf,
    pub(super) args: Vec<String>,
    pub(super) initial_agent_runtime_agent: Option<String>,
}

pub(super) fn requested_workspace_mode(req: &CreateSessionReq) -> &str {
    req.workspace_mode
        .as_deref()
        .unwrap_or_else(|| default_workspace_mode(req))
}

pub(super) fn default_workspace_mode(req: &CreateSessionReq) -> &'static str {
    if req.working_dir.is_some() {
        "main"
    } else if req.launch_agent.is_some()
        || req.resume_session_uuid.is_some()
        || req.claude_resume_uuid.is_some()
    {
        "isolated"
    } else {
        "main"
    }
}

pub(super) fn resolve_session_launch(
    req: &CreateSessionReq,
    repos_root: &StdPath,
) -> ApiResult<SessionLaunch> {
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

    match (e2e_fixture, resume_session_uuid, resume_agent, launch_agent) {
        (Some(crate::e2e::MOCK_TERMINAL_FIXTURE), None, _, None) => {
            resolve_mock_terminal_launch(repos_root)
        }
        (Some(fixture), None, _, None) => Err(ApiError::BadRequest(format!(
            "unknown e2e fixture {fixture}",
        ))),
        (Some(_), Some(_), _, _) => unreachable!("fixture + resume handled above"),
        (Some(_), None, _, Some(_)) => unreachable!("fixture + launch handled above"),
        (None, Some(_), _, Some(_)) => unreachable!("resume + launch handled above"),
        (None, Some(uuid), Some("claude-code" | "claude"), None) => {
            Ok(resume_agent_launch(AgentType::Claude, uuid))
        }
        (None, Some(uuid), Some("codex"), None) => Ok(resume_agent_launch(AgentType::Codex, uuid)),
        (None, Some(_), Some(agent), None) => Err(ApiError::BadRequest(format!(
            "resume is not implemented for agent {agent}",
        ))),
        (None, Some(_), None, None) => Err(ApiError::BadRequest(
            "resume_agent is required when resume_session_uuid is set".into(),
        )),
        (None, None, _, Some(agent)) => Ok(default_agent_launch(agent)),
        (None, None, _, None) => Ok(SessionLaunch {
            shell: crate::pty::default_shell(),
            args: Vec::new(),
            initial_agent_runtime_agent: None,
        }),
    }
}

pub(super) fn resolve_mock_terminal_launch(repos_root: &StdPath) -> ApiResult<SessionLaunch> {
    if !crate::e2e::fixtures_enabled() {
        return Err(ApiError::BadRequest(
            "e2e fixtures are disabled on this backend".into(),
        ));
    }
    let path = crate::e2e::mock_terminal_script_path(repos_root);
    if !path.is_file() {
        return Err(ApiError::Internal(anyhow::anyhow!(
            "mock terminal fixture missing: {}",
            path.display()
        )));
    }
    Ok(SessionLaunch {
        shell: path,
        args: Vec::new(),
        initial_agent_runtime_agent: None,
    })
}

pub(super) fn resume_agent_launch(agent: AgentType, uuid: Uuid) -> SessionLaunch {
    let agent_args = match agent {
        AgentType::Claude => vec![
            "--dangerously-skip-permissions".to_string(),
            "--resume".to_string(),
            uuid.to_string(),
        ],
        AgentType::Codex => vec!["--yolo".to_string(), "resume".to_string(), uuid.to_string()],
    };
    SessionLaunch {
        shell: PathBuf::from("/bin/bash"),
        args: vec![
            "-c".to_string(),
            agent_launch_shell_command(agent, &agent_args, true),
        ],
        initial_agent_runtime_agent: Some(agent.as_str().to_string()),
    }
}

pub(super) fn default_agent_launch(agent: AgentType) -> SessionLaunch {
    SessionLaunch {
        shell: PathBuf::from("/bin/bash"),
        args: vec!["-c".to_string(), default_agent_launch_command(agent, true)],
        initial_agent_runtime_agent: Some(agent.as_str().to_string()),
    }
}

pub(super) async fn resolve_session_workspace(
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

pub(super) fn validate_workspace_request(
    req: &CreateSessionReq,
    workspace_mode: &str,
) -> ApiResult<()> {
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
    if req.working_dir.is_some() && matches!(workspace_mode, "isolated" | "worktree") {
        return Err(ApiError::BadRequest(
            "working_dir is only supported with workspace_mode=main".into(),
        ));
    }
    match workspace_mode {
        "main" | "isolated" | "worktree" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "workspace_mode must be one of: main, isolated".into(),
        )),
    }
}

pub(super) fn repo_path_for_session(state: &AppState, repo: &str) -> ApiResult<PathBuf> {
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

pub(super) fn pty_workspace_metadata(record: &WorkspaceRecord) -> PtyWorkspaceMetadata {
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
