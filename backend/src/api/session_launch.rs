//! Resolving what a session should launch.
//!
//! Which shell, which working directory, which workspace, and which agent a
//! new session starts with. Separated from the routes because it is pure
//! decision-making over the request and the repo layout, with no HTTP or
//! database concerns of its own.

use std::path::Path as StdPath;

use super::routes::{ApiError, ApiResult};
use super::session_routes::{parse_launch_agent, CreateSessionReq};
use crate::node_runtime::SessionLaunch as NodeSessionLaunch;

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

/// Rewrites an absolute `working_dir` as a path relative to its repo, which is
/// what the node resolves against its own checkout.
///
/// The configured `repos_root` is tried first, but an absolute path stored in
/// a session row reflects the layout of the node that hosted it — a control
/// plane that owns no repos (its root points at an empty directory) can still
/// see such paths on resume. The fallback splits on the repo-name component,
/// which is the only part of the layout both sides agree on; the node
/// re-validates whatever relative path comes out against its own checkout.
pub(super) fn protocol_working_dir(
    repos_root: &StdPath,
    repo: &str,
    value: &str,
) -> ApiResult<String> {
    let path = StdPath::new(value);
    if !path.is_absolute() {
        return Ok(value.to_string());
    }
    let repo_root = repos_root.join(repo);
    if let Ok(relative) = path.strip_prefix(&repo_root) {
        return Ok(relative.to_string_lossy().into_owned());
    }
    let mut components = path.components();
    for component in components.by_ref() {
        if component.as_os_str() == repo {
            return Ok(components.as_path().to_string_lossy().into_owned());
        }
    }
    Err(ApiError::BadRequest(format!(
        "absolute working_dir must be inside a {repo} checkout"
    )))
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
    if req.working_dir.is_some() && workspace_mode == "isolated" {
        return Err(ApiError::BadRequest(
            "working_dir is only supported with workspace_mode=main".into(),
        ));
    }
    match workspace_mode {
        "main" | "isolated" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "workspace_mode must be one of: main, isolated".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::protocol_working_dir;
    use std::path::Path;

    #[test]
    fn relative_paths_pass_through() {
        let out = protocol_working_dir(Path::new("/srv/repos"), "app", "sub/dir").unwrap();
        assert_eq!(out, "sub/dir");
    }

    #[test]
    fn absolute_path_under_configured_root_is_stripped() {
        let out =
            protocol_working_dir(Path::new("/srv/repos"), "app", "/srv/repos/app/sub").unwrap();
        assert_eq!(out, "sub");
    }

    #[test]
    fn node_layout_path_falls_back_to_the_repo_component() {
        let out = protocol_working_dir(
            Path::new("/var/empty/sulion/repos"),
            "the-canonry-game",
            "/home/sulion/repos/the-canonry-game",
        )
        .unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn node_layout_subdirectory_keeps_its_tail() {
        let out = protocol_working_dir(
            Path::new("/var/empty/sulion/repos"),
            "app",
            "/home/sulion/repos/app/crates/core",
        )
        .unwrap();
        assert_eq!(out, "crates/core");
    }

    #[test]
    fn a_path_without_the_repo_component_is_rejected() {
        let err = protocol_working_dir(Path::new("/srv/repos"), "app", "/etc/passwd").unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("must be inside"), "unexpected error: {msg}");
    }
}
