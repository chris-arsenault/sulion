use std::sync::Arc;

use uuid::Uuid;

use super::commands::{handle_command, CommandSink};
use super::model::{NodeHello, WireEnvelope, NODE_PROTOCOL_VERSION};
use super::{heartbeat_envelope, NodeControl, NodeHostStats, NodeProtocolError};
use crate::node_runtime::NodeRuntime;

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
                let sink = LoopbackSink {
                    control: &control,
                    connection: &connection,
                    terminal_outbound: runtime_outbound.clone(),
                };
                if let Err(err) = handle_command(&sink, runtime.as_ref(), command).await {
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

/// Replies are handed straight back to the in-process control plane rather
/// than written to a socket, and terminal bytes go through the dedicated
/// channel `run` drains.
struct LoopbackSink<'a> {
    control: &'a NodeControl,
    connection: &'a super::RegisteredConnection,
    terminal_outbound: tokio::sync::mpsc::Sender<WireEnvelope>,
}

impl CommandSink for LoopbackSink<'_> {
    fn node_id(&self) -> Uuid {
        self.connection.node_id
    }

    fn boot_id(&self) -> Uuid {
        self.connection.boot_id
    }

    fn terminal_sender(&self) -> tokio::sync::mpsc::Sender<WireEnvelope> {
        self.terminal_outbound.clone()
    }

    async fn send(&self, envelope: WireEnvelope) -> Result<(), NodeProtocolError> {
        self.control
            .receive_envelope(self.connection.connection_id, envelope)
            .await
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
