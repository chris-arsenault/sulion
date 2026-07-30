//! Wire protocol between the node and a devenv server.
//!
//! Newline-delimited JSON over a unix socket on the shared run volume, the
//! same framing `correlate.rs` uses. The protocol is additive from day one:
//! an old devenv container keeps serving shells against ever-newer nodes, so
//! there is no version handshake — unknown message kinds decode to `Unknown`
//! and are ignored, unknown fields are ignored, optional fields default.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::pty::host::HostSpawnSpec;

/// Messages the node sends to a devenv server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeToDevenv {
    Spawn {
        reply_id: Uuid,
        spec: HostSpawnSpec,
    },
    Input {
        id: Uuid,
        #[serde(default)]
        data: String,
    },
    Resize {
        id: Uuid,
        rows: u16,
        cols: u16,
    },
    Kill {
        id: Uuid,
    },
    SnapshotRequest {
        id: Uuid,
        reply_id: Uuid,
    },
    /// Forward compatibility: a newer node kind this build does not know.
    #[serde(other)]
    Unknown,
}

/// Messages a devenv server sends to the node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DevenvToNode {
    /// First message on every (re)connect: what this devenv is hosting.
    Hello {
        #[serde(default)]
        pid: u32,
        /// Which devenv this is — the image ID of the container it runs in.
        /// Absent for child-mode devenvs and pre-versioning containers.
        #[serde(default)]
        ident: Option<String>,
        #[serde(default)]
        sessions: Vec<InventoryEntry>,
    },
    SpawnResult {
        reply_id: Uuid,
        #[serde(default)]
        ok: bool,
        #[serde(default)]
        error: Option<String>,
    },
    Output {
        id: Uuid,
        #[serde(default)]
        data: String,
    },
    Snapshot {
        reply_id: Uuid,
        #[serde(default)]
        data: String,
    },
    Exited {
        id: Uuid,
        #[serde(default)]
        exit_code: Option<i32>,
    },
    /// Forward compatibility: a newer devenv kind this build does not know.
    #[serde(other)]
    Unknown,
}

/// A session a devenv server is currently hosting. Carries only the id today;
/// additive fields join here as later phases need them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventoryEntry {
    pub id: Uuid,
}

pub fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_bytes(data: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| anyhow::anyhow!("invalid devenv byte payload: {err}"))
}

/// One message per line. The encoded form never contains a raw newline
/// because serde_json escapes control characters inside strings.
pub fn encode_line<T: Serialize>(msg: &T) -> anyhow::Result<String> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    Ok(line)
}

/// Absent-tolerant decode: a line that does not parse is `None`, logged by
/// the caller if it cares. A malformed or future message must never take the
/// connection down.
pub fn decode_line<T: for<'de> Deserialize<'de>>(line: &str) -> Option<T> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_kind_decodes_to_unknown_not_error() {
        let msg: DevenvToNode =
            decode_line(r#"{"kind":"future_thing","id":"x","payload":42}"#).expect("decodes");
        assert!(matches!(msg, DevenvToNode::Unknown));
        let msg: NodeToDevenv = decode_line(r#"{"kind":"upgrade_session"}"#).expect("decodes");
        assert!(matches!(msg, NodeToDevenv::Unknown));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let id = Uuid::new_v4();
        let line = format!(r#"{{"kind":"exited","id":"{id}","exit_code":3,"novel_field":"yes"}}"#);
        let msg: DevenvToNode = decode_line(&line).expect("decodes");
        match msg {
            DevenvToNode::Exited { id: got, exit_code } => {
                assert_eq!(got, id);
                assert_eq!(exit_code, Some(3));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn missing_defaulted_fields_decode() {
        // A phase-1 devenv sends no ident; the hello must still decode.
        let line = r#"{"kind":"hello"}"#;
        let msg: DevenvToNode = decode_line(line).expect("decodes");
        match msg {
            DevenvToNode::Hello {
                pid,
                ident,
                sessions,
            } => {
                assert_eq!(pid, 0);
                assert_eq!(ident, None);
                assert!(sessions.is_empty());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn hello_ident_round_trips() {
        let line = encode_line(&DevenvToNode::Hello {
            pid: 7,
            ident: Some("sha256:abc".into()),
            sessions: Vec::new(),
        })
        .expect("encode");
        match decode_line::<DevenvToNode>(&line).expect("decode") {
            DevenvToNode::Hello { ident, .. } => {
                assert_eq!(ident.as_deref(), Some("sha256:abc"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn garbage_lines_are_none_not_errors() {
        assert!(decode_line::<DevenvToNode>("not json at all").is_none());
        assert!(decode_line::<DevenvToNode>("").is_none());
    }

    #[test]
    fn round_trips_spawn_with_spec() {
        let spec = HostSpawnSpec {
            id: Uuid::new_v4(),
            shell: "/bin/bash".into(),
            args: vec!["-c".into(), "true".into()],
            working_dir: "/".into(),
            env: vec![("A".into(), "b".into())],
            cols: 80,
            rows: 24,
        };
        let reply_id = Uuid::new_v4();
        let line = encode_line(&NodeToDevenv::Spawn {
            reply_id,
            spec: spec.clone(),
        })
        .expect("encode");
        assert!(line.ends_with('\n'));
        let decoded: NodeToDevenv = decode_line(&line).expect("decode");
        match decoded {
            NodeToDevenv::Spawn {
                reply_id: got_reply,
                spec: got_spec,
            } => {
                assert_eq!(got_reply, reply_id);
                assert_eq!(got_spec.id, spec.id);
                assert_eq!(got_spec.env, spec.env);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn bytes_round_trip() {
        let bytes = vec![0u8, 10, 255, 13];
        assert_eq!(decode_bytes(&encode_bytes(&bytes)).unwrap(), bytes);
    }
}
