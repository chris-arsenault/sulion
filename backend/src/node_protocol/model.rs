use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

pub const NODE_PROTOCOL_VERSION: u32 = 1;
pub const CONTROL_PROTOCOL_MIN: u32 = 1;
pub const CONTROL_PROTOCOL_MAX: u32 = 1;
pub const PATH_CONTRACT_VERSION: u32 = 1;
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_HEARTBEAT_TIMEOUT_SECS: u64 = 20;
pub const MAX_NODE_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_REASSEMBLED_MESSAGE_BYTES: usize = 96 * 1024 * 1024;
pub const FRAGMENT_DATA_BYTES: usize = 180 * 1024;
pub const MAX_FRAGMENT_GROUPS: usize = 16;
pub const MAX_FRAGMENTS_PER_MESSAGE: usize = 1024;
pub const MAX_CAPABILITIES: usize = 64;

pub const CAPABILITY_OPERATION_PROBE: &str = "operation.probe.v1";
pub const CAPABILITY_SESSION_RECONCILE: &str = "session.reconcile.v1";
pub const CAPABILITY_SESSION_RUNTIME: &str = "session.runtime.v1";
pub const CAPABILITY_TERMINAL_STREAM: &str = "terminal.stream.v1";
pub const CAPABILITY_REPO_RUNTIME: &str = "repo.runtime.v1";
pub const CAPABILITY_WORKSPACE_RUNTIME: &str = "workspace.runtime.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockerPolicy {
    None,
    Brokered,
    Direct,
}

impl DockerPolicy {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Brokered => "brokered",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockerInfo {
    pub server_version: Option<String>,
    pub rootless: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlChallenge {
    pub challenge_id: Uuid,
    pub nonce: String,
    pub control_build_git_sha: String,
    pub control_protocol_min: u32,
    pub control_protocol_max: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHello {
    pub node_id: Uuid,
    pub boot_id: Uuid,
    pub build_git_sha: String,
    pub protocol_version: u32,
    pub supported_control_min: u32,
    pub supported_control_max: u32,
    pub capabilities: Vec<String>,
    pub docker_policy: DockerPolicy,
    pub docker_info: DockerInfo,
    pub path_contract_version: u32,
    pub observed_release_digest: Option<String>,
    pub signature: String,
}

impl NodeHello {
    pub fn signing_payload(&self, challenge: &ControlChallenge) -> Vec<u8> {
        let mut capabilities = self.capabilities.clone();
        capabilities.sort();
        let release = self.observed_release_digest.as_deref().unwrap_or("");
        let docker_version = self.docker_info.server_version.as_deref().unwrap_or("");
        [
            "sulion-node-handshake-v1".to_string(),
            challenge.challenge_id.to_string(),
            challenge.nonce.clone(),
            challenge.control_build_git_sha.clone(),
            challenge.control_protocol_min.to_string(),
            challenge.control_protocol_max.to_string(),
            self.node_id.to_string(),
            self.boot_id.to_string(),
            self.build_git_sha.clone(),
            self.protocol_version.to_string(),
            self.supported_control_min.to_string(),
            self.supported_control_max.to_string(),
            self.path_contract_version.to_string(),
            self.docker_policy.as_str().to_string(),
            docker_version.to_string(),
            self.docker_info.rootless.to_string(),
            release.to_string(),
            capabilities.join(","),
        ]
        .join("\n")
        .into_bytes()
    }

    pub fn decode_signature(&self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.signature)
            .map_err(|err| anyhow::anyhow!("invalid handshake signature encoding: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    pub accepted: bool,
    pub reason_code: Option<String>,
    pub control_build_git_sha: String,
    pub protocol_version: u32,
    pub accepted_capabilities: Vec<String>,
    pub desired_release_digest: Option<String>,
    pub heartbeat_interval_secs: u64,
    pub heartbeat_timeout_secs: u64,
    pub drain_state: String,
    pub reconciliation: ReconciliationDirective,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationDirective {
    ReportInventory,
    Resume,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireEnvelope {
    pub protocol_version: u32,
    pub node_id: Uuid,
    pub boot_id: Uuid,
    pub message_id: Uuid,
    pub message_kind: String,
    pub request_id: Option<Uuid>,
    pub operation_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub stream_id: Option<Uuid>,
    pub sequence: Option<u64>,
    pub payload: Value,
}

impl WireEnvelope {
    pub fn new(node_id: Uuid, boot_id: Uuid, message_kind: impl Into<String>) -> Self {
        Self {
            protocol_version: NODE_PROTOCOL_VERSION,
            node_id,
            boot_id,
            message_id: Uuid::new_v4(),
            message_kind: message_kind.into(),
            request_id: None,
            operation_id: None,
            session_id: None,
            workspace_id: None,
            stream_id: None,
            sequence: None,
            payload: Value::Object(Default::default()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FragmentPayload {
    index: usize,
    total: usize,
    data: String,
}

struct FragmentGroup {
    node_id: Uuid,
    boot_id: Uuid,
    total: usize,
    parts: Vec<Option<Vec<u8>>>,
    bytes: usize,
}

#[derive(Default)]
pub struct FragmentAssembler {
    groups: HashMap<Uuid, FragmentGroup>,
    bytes: usize,
}

impl FragmentAssembler {
    pub fn push(&mut self, envelope: WireEnvelope) -> anyhow::Result<Option<WireEnvelope>> {
        if envelope.message_kind != "protocol.fragment" {
            return Ok(Some(envelope));
        }
        let payload: FragmentPayload = serde_json::from_value(envelope.payload)?;
        if payload.total == 0
            || payload.total > MAX_FRAGMENTS_PER_MESSAGE
            || payload.index >= payload.total
        {
            anyhow::bail!("invalid node protocol fragment index");
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(payload.data)
            .map_err(|error| anyhow::anyhow!("invalid node protocol fragment: {error}"))?;
        if data.len() > FRAGMENT_DATA_BYTES {
            anyhow::bail!("node protocol fragment exceeds chunk limit");
        }
        if !self.groups.contains_key(&envelope.message_id)
            && self.groups.len() >= MAX_FRAGMENT_GROUPS
        {
            anyhow::bail!("too many incomplete node protocol messages");
        }
        let group = self
            .groups
            .entry(envelope.message_id)
            .or_insert_with(|| FragmentGroup {
                node_id: envelope.node_id,
                boot_id: envelope.boot_id,
                total: payload.total,
                parts: vec![None; payload.total],
                bytes: 0,
            });
        if group.node_id != envelope.node_id
            || group.boot_id != envelope.boot_id
            || group.total != payload.total
        {
            anyhow::bail!("node protocol fragment group changed identity");
        }
        if group.parts[payload.index].is_none() {
            if self.bytes.saturating_add(data.len()) > MAX_REASSEMBLED_MESSAGE_BYTES {
                let removed = self.groups.remove(&envelope.message_id);
                if let Some(removed) = removed {
                    self.bytes = self.bytes.saturating_sub(removed.bytes);
                }
                anyhow::bail!("incomplete node protocol messages exceed aggregate limit");
            }
            group.bytes += data.len();
            self.bytes += data.len();
            group.parts[payload.index] = Some(data);
        }
        if group.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let group = self
            .groups
            .remove(&envelope.message_id)
            .expect("complete fragment group exists");
        self.bytes = self.bytes.saturating_sub(group.bytes);
        let mut serialized = Vec::with_capacity(group.bytes);
        for part in group.parts {
            serialized.extend(part.expect("complete fragment group has every part"));
        }
        Ok(Some(serde_json::from_slice(&serialized)?))
    }
}

pub fn fragment_envelope(envelope: &WireEnvelope) -> anyhow::Result<Vec<WireEnvelope>> {
    let serialized = serde_json::to_vec(envelope)?;
    if serialized.len() <= FRAGMENT_DATA_BYTES {
        return Ok(vec![envelope.clone()]);
    }
    if serialized.len() > MAX_REASSEMBLED_MESSAGE_BYTES {
        anyhow::bail!("node protocol message exceeds reassembly limit");
    }
    let total = serialized.len().div_ceil(FRAGMENT_DATA_BYTES);
    if total > MAX_FRAGMENTS_PER_MESSAGE {
        anyhow::bail!("node protocol message requires too many fragments");
    }
    let group_id = envelope.message_id;
    serialized
        .chunks(FRAGMENT_DATA_BYTES)
        .enumerate()
        .map(|(index, bytes)| {
            let mut fragment =
                WireEnvelope::new(envelope.node_id, envelope.boot_id, "protocol.fragment");
            fragment.message_id = group_id;
            fragment.sequence = Some(index as u64);
            fragment.payload = serde_json::to_value(FragmentPayload {
                index,
                total,
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            })?;
            Ok(fragment)
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlWireMessage {
    Challenge { challenge: ControlChallenge },
    Envelope { envelope: WireEnvelope },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeWireMessage {
    Hello {
        envelope: WireEnvelope,
        hello: NodeHello,
    },
    Envelope {
        envelope: WireEnvelope,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeOperationKind {
    ProbeEcho,
    ReconcileInventory,
    SessionCreate,
    SessionDelete,
    SessionAgentStart,
    SessionAgentInterrupt,
    RepoCreate,
    RepoRename,
    RepoDelete,
    RepoRefresh,
    RepoStage,
    RepoUpload,
    WorkspaceDelete,
    WorkspaceRefresh,
    WorkspaceStage,
    WorkspaceUpload,
}

impl NodeOperationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ProbeEcho => "probe_echo",
            Self::ReconcileInventory => "reconcile_inventory",
            Self::SessionCreate => "session_create",
            Self::SessionDelete => "session_delete",
            Self::SessionAgentStart => "session_agent_start",
            Self::SessionAgentInterrupt => "session_agent_interrupt",
            Self::RepoCreate => "repo_create",
            Self::RepoRename => "repo_rename",
            Self::RepoDelete => "repo_delete",
            Self::RepoRefresh => "repo_refresh",
            Self::RepoStage => "repo_stage",
            Self::RepoUpload => "repo_upload",
            Self::WorkspaceDelete => "workspace_delete",
            Self::WorkspaceRefresh => "workspace_refresh",
            Self::WorkspaceStage => "workspace_stage",
            Self::WorkspaceUpload => "workspace_upload",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "probe_echo" => Self::ProbeEcho,
            "reconcile_inventory" => Self::ReconcileInventory,
            "session_create" => Self::SessionCreate,
            "session_delete" => Self::SessionDelete,
            "session_agent_start" => Self::SessionAgentStart,
            "session_agent_interrupt" => Self::SessionAgentInterrupt,
            "repo_create" => Self::RepoCreate,
            "repo_rename" => Self::RepoRename,
            "repo_delete" => Self::RepoDelete,
            "repo_refresh" => Self::RepoRefresh,
            "repo_stage" => Self::RepoStage,
            "repo_upload" => Self::RepoUpload,
            "workspace_delete" => Self::WorkspaceDelete,
            "workspace_refresh" => Self::WorkspaceRefresh,
            "workspace_stage" => Self::WorkspaceStage,
            "workspace_upload" => Self::WorkspaceUpload,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRequestKind {
    SessionInput,
    SessionResize,
    RepoRefresh,
    RepoStage,
    RepoUpload,
    RepoFiles,
    RepoFilePreview,
    RepoFileRaw,
    RepoDiff,
    RepoDirtyPaths,
    WorkspaceRefresh,
    WorkspaceStage,
    WorkspaceUpload,
    WorkspaceFiles,
    WorkspaceFilePreview,
    WorkspaceFileRaw,
    WorkspaceDiff,
    WorkspaceDirtyPaths,
}

impl NodeRequestKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::SessionInput => "session_input",
            Self::SessionResize => "session_resize",
            Self::RepoRefresh => "repo_refresh",
            Self::RepoStage => "repo_stage",
            Self::RepoUpload => "repo_upload",
            Self::RepoFiles => "repo_files",
            Self::RepoFilePreview => "repo_file_preview",
            Self::RepoFileRaw => "repo_file_raw",
            Self::RepoDiff => "repo_diff",
            Self::RepoDirtyPaths => "repo_dirty_paths",
            Self::WorkspaceRefresh => "workspace_refresh",
            Self::WorkspaceStage => "workspace_stage",
            Self::WorkspaceUpload => "workspace_upload",
            Self::WorkspaceFiles => "workspace_files",
            Self::WorkspaceFilePreview => "workspace_file_preview",
            Self::WorkspaceFileRaw => "workspace_file_raw",
            Self::WorkspaceDiff => "workspace_diff",
            Self::WorkspaceDirtyPaths => "workspace_dirty_paths",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "session_input" => Self::SessionInput,
            "session_resize" => Self::SessionResize,
            "repo_refresh" => Self::RepoRefresh,
            "repo_stage" => Self::RepoStage,
            "repo_upload" => Self::RepoUpload,
            "repo_files" => Self::RepoFiles,
            "repo_file_preview" => Self::RepoFilePreview,
            "repo_file_raw" => Self::RepoFileRaw,
            "repo_diff" => Self::RepoDiff,
            "repo_dirty_paths" => Self::RepoDirtyPaths,
            "workspace_refresh" => Self::WorkspaceRefresh,
            "workspace_stage" => Self::WorkspaceStage,
            "workspace_upload" => Self::WorkspaceUpload,
            "workspace_files" => Self::WorkspaceFiles,
            "workspace_file_preview" => Self::WorkspaceFilePreview,
            "workspace_file_raw" => Self::WorkspaceFileRaw,
            "workspace_diff" => Self::WorkspaceDiff,
            "workspace_dirty_paths" => Self::WorkspaceDirtyPaths,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestResultPayload {
    pub status: OperationResultStatus,
    pub result: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalBytesPayload {
    pub data: String,
}

impl TerminalBytesPayload {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::STANDARD
            .decode(self.data)
            .map_err(|err| anyhow::anyhow!("invalid terminal byte payload: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalResizePayload {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalDeadPayload {
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResultPayload {
    pub status: OperationResultStatus,
    pub result: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationResultStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NodeView {
    pub id: Uuid,
    pub display_name: String,
    pub credential_status: String,
    pub protocol_version: Option<i32>,
    pub build_git_sha: Option<String>,
    pub capabilities: Value,
    pub docker_policy: String,
    pub docker_info: Value,
    pub path_contract_version: Option<i32>,
    pub boot_id: Option<Uuid>,
    pub connection_state: String,
    pub compatibility_error: Option<String>,
    pub desired_release_digest: Option<String>,
    pub observed_release_digest: Option<String>,
    pub drain_state: String,
    pub connected_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub node_disconnected_at: Option<DateTime<Utc>>,
    pub heartbeat_timeout_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct NodeOperationView {
    pub operation_id: Uuid,
    pub idempotency_key: String,
    pub node_id: Uuid,
    pub kind: String,
    pub resource_id: Option<Uuid>,
    pub request_payload: Value,
    pub requested_at: DateTime<Utc>,
    pub dispatched_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: String,
    pub result: Option<Value>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub dispatch_boot_id: Option<Uuid>,
    pub dispatch_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub target_node_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateEnrollmentTokenRequest {
    pub display_name: String,
    pub target_node_id: Option<Uuid>,
    pub ttl_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnrollNodeRequest {
    pub token: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollNodeResponse {
    pub node_id: Uuid,
    pub display_name: String,
    pub credential_generation: i32,
    pub protocol_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge() -> ControlChallenge {
        ControlChallenge {
            challenge_id: Uuid::nil(),
            nonce: "nonce".into(),
            control_build_git_sha: "control".into(),
            control_protocol_min: 1,
            control_protocol_max: 1,
        }
    }

    fn hello() -> NodeHello {
        NodeHello {
            node_id: Uuid::nil(),
            boot_id: Uuid::nil(),
            build_git_sha: "node".into(),
            protocol_version: 1,
            supported_control_min: 1,
            supported_control_max: 1,
            capabilities: vec!["z.v1".into(), "a.v1".into()],
            docker_policy: DockerPolicy::Direct,
            docker_info: DockerInfo {
                server_version: Some("27.0".into()),
                rootless: true,
            },
            path_contract_version: 1,
            observed_release_digest: Some("sha256:test".into()),
            signature: String::new(),
        }
    }

    #[test]
    fn handshake_signature_is_capability_order_independent() {
        let first = hello();
        let mut reordered = first.clone();
        reordered.capabilities.reverse();
        assert_eq!(
            first.signing_payload(&challenge()),
            reordered.signing_payload(&challenge())
        );
    }

    #[test]
    fn handshake_signature_covers_docker_daemon_facts() {
        let first = hello();
        let mut changed = first.clone();
        changed.docker_info.rootless = false;
        assert_ne!(
            first.signing_payload(&challenge()),
            changed.signing_payload(&challenge())
        );
    }

    #[test]
    fn large_envelopes_round_trip_through_bounded_fragments() {
        let mut original = WireEnvelope::new(Uuid::new_v4(), Uuid::new_v4(), "request.result");
        original.request_id = Some(Uuid::new_v4());
        original.payload = serde_json::json!({
            "data": "x".repeat(FRAGMENT_DATA_BYTES * 3),
        });

        let fragments = fragment_envelope(&original).expect("fragment message");
        assert!(fragments.len() > 1);
        let mut assembler = FragmentAssembler::default();
        let mut reassembled = None;
        for fragment in fragments {
            let wire = NodeWireMessage::Envelope {
                envelope: fragment.clone(),
            };
            assert!(
                serde_json::to_vec(&wire).unwrap().len() <= MAX_NODE_FRAME_BYTES,
                "fragment must fit the WebSocket frame limit"
            );
            reassembled = assembler.push(fragment).expect("accept fragment");
        }

        let reassembled = reassembled.expect("last fragment completes message");
        assert_eq!(
            serde_json::to_value(reassembled).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }
}
