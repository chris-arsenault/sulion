use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::model::{
    NodeHello, NodeRequestKind, RequestResultPayload, RequestResultStatus, TerminalBytesPayload,
    TerminalResizePayload, WireEnvelope, NODE_PROTOCOL_VERSION,
};
use super::{heartbeat_envelope, NodeControl, NodeHostStats, NodeProtocolError};
use crate::node_runtime::{NodeRuntime, SessionInputRequest, SessionResizeRequest};

#[derive(Debug, Deserialize)]
struct EphemeralRequest {
    kind: String,
    request: Value,
}

pub(super) async fn start(
    control: Arc<NodeControl>,
    node_id: Uuid,
    display_name: &str,
) -> Result<Uuid, NodeProtocolError> {
    control
        .store
        .ensure_internal_node(node_id, display_name)
        .await?;
    let boot_id = Uuid::new_v4();
    let hello = NodeHello {
        node_id,
        boot_id,
        protocol_version: NODE_PROTOCOL_VERSION,
        node_nonce: String::new(),
        public_key: None,
        signature: String::new(),
    };
    let registered = control.register_internal(hello).await?;
    tokio::spawn(run(control, registered, None));
    Ok(boot_id)
}

pub(super) async fn start_runtime(
    control: Arc<NodeControl>,
    runtime: Arc<NodeRuntime>,
    display_name: &str,
) -> Result<Uuid, NodeProtocolError> {
    control
        .store
        .ensure_internal_node(runtime.node_id(), display_name)
        .await?;
    let hello = NodeHello {
        node_id: runtime.node_id(),
        boot_id: runtime.boot_id(),
        protocol_version: NODE_PROTOCOL_VERSION,
        node_nonce: String::new(),
        public_key: None,
        signature: String::new(),
    };
    let registered = control.register_internal(hello).await?;
    let boot_id = registered.boot_id;
    tokio::spawn(run(control, registered, Some(runtime)));
    Ok(boot_id)
}

async fn run(
    control: Arc<NodeControl>,
    mut connection: super::RegisteredConnection,
    runtime: Option<Arc<NodeRuntime>>,
) {
    let initial = heartbeat_envelope(
        connection.node_id,
        connection.boot_id,
        live_sessions(&runtime).await,
        true,
        host_stats(&runtime).await,
    );
    if control
        .receive_envelope(connection.connection_id, initial)
        .await
        .is_err()
    {
        control
            .disconnected(
                connection.node_id,
                connection.boot_id,
                connection.connection_id,
            )
            .await;
        return;
    }

    let (runtime_outbound, mut runtime_inbound) = tokio::sync::mpsc::channel(256);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(
        connection.ack.heartbeat_interval_secs,
    ));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            changed = connection.canceled.changed() => {
                if changed.is_err() || *connection.canceled.borrow() {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                let envelope = heartbeat_envelope(
                    connection.node_id,
                    connection.boot_id,
                    live_sessions(&runtime).await,
                    true,
                    host_stats(&runtime).await,
                );
                if control.receive_envelope(connection.connection_id, envelope).await.is_err() {
                    break;
                }
            }
            command = connection.outbound.recv() => {
                let Some(command) = command else {
                    break;
                };
                if let Err(err) = handle_command(
                    &control,
                    &connection,
                    runtime.as_ref(),
                    &runtime_outbound,
                    command,
                ).await {
                    tracing::warn!(%err, node_id = %connection.node_id, "loopback node command failed");
                    break;
                }
            }
            event = runtime_inbound.recv() => {
                let Some(event) = event else {
                    continue;
                };
                if control.receive_envelope(connection.connection_id, event).await.is_err() {
                    break;
                }
            }
        }
    }
    control
        .disconnected(
            connection.node_id,
            connection.boot_id,
            connection.connection_id,
        )
        .await;
}

async fn handle_command(
    control: &NodeControl,
    connection: &super::RegisteredConnection,
    runtime: Option<&Arc<NodeRuntime>>,
    runtime_outbound: &tokio::sync::mpsc::Sender<WireEnvelope>,
    command: WireEnvelope,
) -> Result<(), NodeProtocolError> {
    match command.message_kind.as_str() {
        "request" => {
            let request_id = command.request_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("request missing request_id".into())
            })?;
            let request: EphemeralRequest = serde_json::from_value(command.payload)?;
            let result = match (runtime, NodeRequestKind::parse(&request.kind)) {
                (Some(runtime), Some(kind)) => runtime.execute_request(kind, request.request).await,
                (None, Some(NodeRequestKind::ProbeEcho)) => RequestResultPayload {
                    status: RequestResultStatus::Succeeded,
                    result: Some(json!({ "echo": request.request })),
                    error_code: None,
                    error_message: None,
                },
                _ => RequestResultPayload {
                    status: RequestResultStatus::Failed,
                    result: None,
                    error_code: Some("unsupported_request".into()),
                    error_message: Some("request is not supported by this node".into()),
                },
            };
            let mut envelope =
                WireEnvelope::new(connection.node_id, connection.boot_id, "request.result");
            envelope.request_id = Some(request_id);
            envelope.payload = serde_json::to_value(result)?;
            control
                .receive_envelope(connection.connection_id, envelope)
                .await
        }
        "terminal.attach" => {
            let runtime = runtime.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal runtime is unavailable".into())
            })?;
            runtime
                .open_terminal(
                    command.stream_id.ok_or_else(|| {
                        NodeProtocolError::InvalidRequest(
                            "terminal attach missing stream_id".into(),
                        )
                    })?,
                    command.session_id.ok_or_else(|| {
                        NodeProtocolError::InvalidRequest(
                            "terminal attach missing session_id".into(),
                        )
                    })?,
                    runtime_outbound.clone(),
                )
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
            let result = runtime
                .execute_request(
                    NodeRequestKind::SessionInput,
                    serde_json::to_value(SessionInputRequest::from_bytes(session_id, &bytes))?,
                )
                .await;
            ensure_request_succeeded(result)
        }
        "terminal.resize" => {
            let runtime = runtime.ok_or(NodeProtocolError::Unavailable)?;
            let session_id = command.session_id.ok_or_else(|| {
                NodeProtocolError::InvalidRequest("terminal resize missing session_id".into())
            })?;
            let resize: TerminalResizePayload = serde_json::from_value(command.payload)?;
            let result = runtime
                .execute_request(
                    NodeRequestKind::SessionResize,
                    serde_json::to_value(SessionResizeRequest {
                        session_id,
                        cols: resize.cols,
                        rows: resize.rows,
                    })?,
                )
                .await;
            ensure_request_succeeded(result)
        }
        _ => Ok(()),
    }
}

async fn live_sessions(runtime: &Option<Arc<NodeRuntime>>) -> Vec<Uuid> {
    match runtime {
        Some(runtime) => runtime.live_session_ids().await,
        None => Vec::new(),
    }
}

/// Standalone runs the node in this process, so its machine is the node's
/// machine and the same sample is the right one to report. A loopback
/// connection with no runtime owns nothing and reports nothing.
async fn host_stats(runtime: &Option<Arc<NodeRuntime>>) -> Option<NodeHostStats> {
    match runtime {
        Some(runtime) => Some(runtime.host_stats().await),
        None => None,
    }
}

fn ensure_request_succeeded(result: RequestResultPayload) -> Result<(), NodeProtocolError> {
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
