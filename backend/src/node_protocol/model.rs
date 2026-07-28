use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

pub const NODE_PROTOCOL_VERSION: u32 = 1;
pub const DEDICATED_NODE_ID: Uuid = Uuid::from_u128(0x019d4f28_88ac_7a80_932c_b0f53a0708f4);
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 5;
pub const DEFAULT_HEARTBEAT_TIMEOUT_SECS: u64 = 20;
pub const MAX_NODE_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_REASSEMBLED_MESSAGE_BYTES: usize = 96 * 1024 * 1024;
pub const FRAGMENT_DATA_BYTES: usize = 180 * 1024;
pub const MAX_FRAGMENT_GROUPS: usize = 16;
pub const MAX_FRAGMENTS_PER_MESSAGE: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlChallenge {
    pub challenge_id: Uuid,
    pub nonce: String,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHello {
    pub node_id: Uuid,
    pub boot_id: Uuid,
    pub protocol_version: u32,
    #[serde(default)]
    pub public_key: Option<String>,
    pub signature: String,
}

impl NodeHello {
    pub fn signing_payload(&self, challenge: &ControlChallenge) -> Vec<u8> {
        let mut fields = vec![
            "sulion-node-handshake-v1".to_string(),
            challenge.challenge_id.to_string(),
            challenge.nonce.clone(),
            challenge.protocol_version.to_string(),
            self.node_id.to_string(),
            self.boot_id.to_string(),
            self.protocol_version.to_string(),
        ];
        if let Some(public_key) = &self.public_key {
            fields.push(public_key.clone());
        }
        fields.join("\n").into_bytes()
    }

    pub fn decode_signature(&self) -> anyhow::Result<Vec<u8>> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.signature)
            .map_err(|err| anyhow::anyhow!("invalid handshake signature encoding: {err}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloAck {
    pub protocol_version: u32,
    pub heartbeat_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireEnvelope {
    pub protocol_version: u32,
    pub node_id: Uuid,
    pub boot_id: Uuid,
    pub message_id: Uuid,
    pub message_kind: String,
    pub request_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
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
            session_id: None,
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
pub enum NodeRequestKind {
    ProbeEcho,
    SessionCreate,
    SessionDelete,
    SessionAgentStart,
    SessionAgentInterrupt,
    SessionInput,
    SessionResize,
    RepoCreate,
    RepoRename,
    RepoDelete,
    RepoRefresh,
    RepoStage,
    RepoUpload,
    RepoFiles,
    RepoFilePreview,
    RepoFileRaw,
    RepoDiff,
    RepoDirtyPaths,
    WorkspaceDelete,
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
            Self::ProbeEcho => "probe_echo",
            Self::SessionCreate => "session_create",
            Self::SessionDelete => "session_delete",
            Self::SessionAgentStart => "session_agent_start",
            Self::SessionAgentInterrupt => "session_agent_interrupt",
            Self::SessionInput => "session_input",
            Self::SessionResize => "session_resize",
            Self::RepoCreate => "repo_create",
            Self::RepoRename => "repo_rename",
            Self::RepoDelete => "repo_delete",
            Self::RepoRefresh => "repo_refresh",
            Self::RepoStage => "repo_stage",
            Self::RepoUpload => "repo_upload",
            Self::RepoFiles => "repo_files",
            Self::RepoFilePreview => "repo_file_preview",
            Self::RepoFileRaw => "repo_file_raw",
            Self::RepoDiff => "repo_diff",
            Self::RepoDirtyPaths => "repo_dirty_paths",
            Self::WorkspaceDelete => "workspace_delete",
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
            "probe_echo" => Self::ProbeEcho,
            "session_create" => Self::SessionCreate,
            "session_delete" => Self::SessionDelete,
            "session_agent_start" => Self::SessionAgentStart,
            "session_agent_interrupt" => Self::SessionAgentInterrupt,
            "session_input" => Self::SessionInput,
            "session_resize" => Self::SessionResize,
            "repo_create" => Self::RepoCreate,
            "repo_rename" => Self::RepoRename,
            "repo_delete" => Self::RepoDelete,
            "repo_refresh" => Self::RepoRefresh,
            "repo_stage" => Self::RepoStage,
            "repo_upload" => Self::RepoUpload,
            "repo_files" => Self::RepoFiles,
            "repo_file_preview" => Self::RepoFilePreview,
            "repo_file_raw" => Self::RepoFileRaw,
            "repo_diff" => Self::RepoDiff,
            "repo_dirty_paths" => Self::RepoDirtyPaths,
            "workspace_delete" => Self::WorkspaceDelete,
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
    pub status: RequestResultStatus,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestResultStatus {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeView {
    pub id: Uuid,
    pub display_name: String,
    pub protocol_version: Option<i32>,
    pub boot_id: Option<Uuid>,
    pub connection_state: String,
    pub connected_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub node_disconnected_at: Option<DateTime<Utc>>,
    pub heartbeat_timeout_seconds: i64,
    pub pending_key_fingerprint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge() -> ControlChallenge {
        ControlChallenge {
            challenge_id: Uuid::nil(),
            nonce: "nonce".into(),
            protocol_version: 1,
        }
    }

    fn hello() -> NodeHello {
        NodeHello {
            node_id: Uuid::nil(),
            boot_id: Uuid::nil(),
            protocol_version: 1,
            public_key: Some("public-key".into()),
            signature: String::new(),
        }
    }

    #[test]
    fn handshake_signature_covers_boot_identity() {
        let first = hello();
        let mut changed = first.clone();
        changed.boot_id = Uuid::new_v4();
        assert_ne!(
            first.signing_payload(&challenge()),
            changed.signing_payload(&challenge())
        );

        let mut changed = first.clone();
        changed.public_key = Some("replacement-key".into());
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
