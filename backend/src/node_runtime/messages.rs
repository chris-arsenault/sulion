//! Machine-local development runtime.
//!
//! This module is the ownership boundary behind the development-node
//! protocol. It is deliberately usable by both the extracted `sulion-node`
//! process and the transitional in-memory transport: API handlers never need
//! a repo path or PTY handle when node mode is enabled.

use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionLaunch {
    Shell,
    Agent {
        agent: String,
        resume_session_uuid: Option<Uuid>,
    },
    MockTerminal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreateRequest {
    pub session_id: Uuid,
    pub allocated_workspace_id: Uuid,
    pub existing_workspace_id: Option<Uuid>,
    pub repo: String,
    pub working_dir: Option<String>,
    pub workspace_mode: String,
    pub cols: u16,
    pub rows: u16,
    pub launch: SessionLaunch,
    #[serde(default)]
    pub meta_repo: Option<SessionMetaRepoRequest>,
    #[serde(default)]
    pub additional_repos: Vec<SessionRepoRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetaRepoRequest {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRepoRequest {
    pub repo: String,
    pub allocated_workspace_id: Uuid,
    pub existing_workspace_id: Option<Uuid>,
    pub position: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub session_id: Uuid,
    pub agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRequest {
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCreateRequest {
    pub name: String,
    pub git_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoRenameRequest {
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoDeleteRequest {
    pub name: String,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoPathRequest {
    pub repo: String,
    pub path: Option<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspacePathRequest {
    pub workspace_id: Uuid,
    pub path: Option<String>,
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRequest {
    pub path: String,
    pub stage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStageRequest {
    pub repo: String,
    pub change: StageRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceStageRequest {
    pub workspace_id: Uuid,
    pub change: StageRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRequest {
    pub path: String,
    pub data: String,
}

impl UploadRequest {
    pub fn new(path: String, bytes: &[u8]) -> Self {
        Self {
            path,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub(super) fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid upload payload: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoUploadRequest {
    pub repo: String,
    pub upload: UploadRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceUploadRequest {
    pub workspace_id: Uuid,
    pub upload: UploadRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFileResponse {
    pub path: String,
    pub size: usize,
    pub data: String,
}

impl RawFileResponse {
    pub fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid raw file payload: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInputRequest {
    pub session_id: Uuid,
    pub data: String,
}

impl SessionInputRequest {
    pub fn from_bytes(session_id: Uuid, bytes: &[u8]) -> Self {
        Self {
            session_id,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub(super) fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid session input: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResizeRequest {
    pub session_id: Uuid,
    pub cols: u16,
    pub rows: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_single_repo_session_request_keeps_parsing() {
        let request: SessionCreateRequest = serde_json::from_value(serde_json::json!({
            "session_id": Uuid::new_v4(),
            "allocated_workspace_id": Uuid::new_v4(),
            "existing_workspace_id": null,
            "repo": "app",
            "working_dir": null,
            "workspace_mode": "main",
            "cols": 120,
            "rows": 32,
            "launch": {"kind": "shell"}
        }))
        .expect("old request parses");

        assert!(request.meta_repo.is_none());
        assert!(request.additional_repos.is_empty());
    }
}
