//! One implementation of the control-plane command handlers, shared by the
//! websocket client (`client.rs`) and the in-process loopback (`loopback.rs`).
//!
//! The two transports differ only in where replies go and whether a runtime is
//! attached at all, so both are expressed through [`CommandSink`] and an
//! `Option<&Arc<NodeRuntime>>`. Before this existed the handlers were written
//! twice and had already drifted — different error text, and the real client
//! spawning commands concurrently where loopback awaited them inline — while
//! only the loopback copy was covered by integration tests.

use std::future::Future;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::model::{
    NodeRequestKind, RequestResultPayload, RequestResultStatus, TerminalBytesPayload,
    TerminalResizePayload, WireEnvelope,
};
use super::NodeProtocolError;
use crate::node_runtime::{NodeRuntime, SessionInputRequest, SessionResizeRequest};

#[derive(Debug, Deserialize)]
pub(super) struct EphemeralRequest {
    pub kind: String,
    pub request: Value,
}

/// Where a node's replies go, and the identity they are stamped with.
pub(super) trait CommandSink {
    fn node_id(&self) -> Uuid;
    fn boot_id(&self) -> Uuid;

    /// Channel `open_terminal` writes terminal byte events to. Separate from
    /// [`CommandSink::send`] because loopback drains terminal events through a
    /// dedicated channel rather than replying inline.
    fn terminal_sender(&self) -> mpsc::Sender<WireEnvelope>;

    fn send(
        &self,
        envelope: WireEnvelope,
    ) -> impl Future<Output = Result<(), NodeProtocolError>> + Send;
}

/// Handles one command addressed to a node.
///
/// `runtime` is `None` for a loopback connection registered without one, which
/// owns no sessions and can answer only a probe. Unknown message kinds are
/// ignored rather than refused, so a control plane that has learned a newer
/// command cannot break an older node.
pub(super) async fn handle_command<S>(
    sink: &S,
    runtime: Option<&Arc<NodeRuntime>>,
    command: WireEnvelope,
) -> Result<(), NodeProtocolError>
where
    S: CommandSink + Sync,
{
    match command.message_kind.as_str() {
        "request" => {
            let request_id = command.request_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("request missing request_id".into())
            })?;
            let request: EphemeralRequest = serde_json::from_value(command.payload)?;
            let result = match (runtime, NodeRequestKind::parse(&request.kind)) {
                (Some(runtime), Some(kind)) => runtime.execute_request(kind, request.request).await,
                // A runtime-less loopback still answers a probe, which is how
                // the control plane checks the channel is alive.
                (None, Some(NodeRequestKind::ProbeEcho)) => RequestResultPayload {
                    status: RequestResultStatus::Succeeded,
                    result: Some(json!({ "echo": request.request })),
                    error_code: None,
                    error_message: None,
                },
                (None, _) => unsupported("request is not supported by this node"),
                (Some(_), None) => unsupported("request is not supported by this node release"),
            };
            let mut envelope = WireEnvelope::new(sink.node_id(), sink.boot_id(), "request.result");
            envelope.request_id = Some(request_id);
            envelope.payload = serde_json::to_value(result)?;
            sink.send(envelope).await
        }
        "terminal.attach" => {
            let runtime = runtime.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal runtime is unavailable".into())
            })?;
            let stream_id = command.stream_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal attach missing stream_id".into())
            })?;
            let session_id = command.session_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal attach missing session_id".into())
            })?;
            runtime
                .open_terminal(stream_id, session_id, sink.terminal_sender())
                .await
                .map_err(|err| NodeProtocolError::InvalidRequest(err.to_string()))
        }
        "terminal.detach" => {
            if let (Some(runtime), Some(stream_id)) = (runtime, command.stream_id) {
                runtime.close_terminal(stream_id).await;
            }
            Ok(())
        }
        "terminal.input" => {
            let runtime = runtime.ok_or(NodeProtocolError::Unavailable)?;
            let session_id = command.session_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal input missing session_id".into())
            })?;
            let bytes = serde_json::from_value::<TerminalBytesPayload>(command.payload)?
                .into_bytes()
                .map_err(|err| NodeProtocolError::InvalidRequest(err.to_string()))?;
            ensure_request_succeeded(
                runtime
                    .execute_request(
                        NodeRequestKind::SessionInput,
                        serde_json::to_value(SessionInputRequest::from_bytes(session_id, &bytes))?,
                    )
                    .await,
            )
        }
        "terminal.resize" => {
            let runtime = runtime.ok_or(NodeProtocolError::Unavailable)?;
            let session_id = command.session_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal resize missing session_id".into())
            })?;
            let resize: TerminalResizePayload = serde_json::from_value(command.payload)?;
            ensure_request_succeeded(
                runtime
                    .execute_request(
                        NodeRequestKind::SessionResize,
                        serde_json::to_value(SessionResizeRequest {
                            session_id,
                            cols: resize.cols,
                            rows: resize.rows,
                        })?,
                    )
                    .await,
            )
        }
        _ => {
            tracing::debug!(kind = %command.message_kind, "ignoring unknown control message");
            Ok(())
        }
    }
}

fn unsupported(message: &str) -> RequestResultPayload {
    RequestResultPayload {
        status: RequestResultStatus::Failed,
        result: None,
        error_code: Some("unsupported_request".into()),
        error_message: Some(message.to_string()),
    }
}

pub(super) fn ensure_request_succeeded(
    result: RequestResultPayload,
) -> Result<(), NodeProtocolError> {
    match result.status {
        RequestResultStatus::Succeeded => Ok(()),
        RequestResultStatus::Failed => Err(NodeProtocolError::Remote {
            code: result.error_code.unwrap_or_else(|| "request_failed".into()),
            message: result
                .error_message
                .unwrap_or_else(|| "node request failed".into()),
        }),
    }
}
