use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::model::{
    DockerInfo, DockerPolicy, NodeHello, OperationResultPayload, OperationResultStatus,
    WireEnvelope, CAPABILITY_OPERATION_PROBE, CAPABILITY_SESSION_RECONCILE, CONTROL_PROTOCOL_MAX,
    CONTROL_PROTOCOL_MIN, NODE_PROTOCOL_VERSION, PATH_CONTRACT_VERSION,
};
use super::{heartbeat_envelope, operation_result_envelope, NodeControl, NodeProtocolError};

const RESULT_CACHE_CAPACITY: usize = 256;

#[derive(Debug, Deserialize)]
struct OperationRequest {
    kind: String,
    request: Value,
}

struct ResultCache {
    values: HashMap<Uuid, OperationResultPayload>,
    order: VecDeque<Uuid>,
}

impl ResultCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, operation_id: Uuid) -> Option<OperationResultPayload> {
        self.values.get(&operation_id).cloned()
    }

    fn insert(&mut self, operation_id: Uuid, result: OperationResultPayload) {
        if self.values.contains_key(&operation_id) {
            return;
        }
        self.values.insert(operation_id, result);
        self.order.push_back(operation_id);
        while self.order.len() > RESULT_CACHE_CAPACITY {
            if let Some(expired) = self.order.pop_front() {
                self.values.remove(&expired);
            }
        }
    }
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
        build_git_sha: "standalone-loopback".into(),
        protocol_version: NODE_PROTOCOL_VERSION,
        supported_control_min: CONTROL_PROTOCOL_MIN,
        supported_control_max: CONTROL_PROTOCOL_MAX,
        capabilities: vec![
            CAPABILITY_OPERATION_PROBE.into(),
            CAPABILITY_SESSION_RECONCILE.into(),
        ],
        docker_policy: DockerPolicy::None,
        docker_info: DockerInfo {
            server_version: None,
            rootless: false,
        },
        path_contract_version: PATH_CONTRACT_VERSION,
        observed_release_digest: None,
        signature: String::new(),
    };
    let registered = control.register_internal(hello).await?;
    tokio::spawn(run(control, registered));
    Ok(boot_id)
}

async fn run(control: Arc<NodeControl>, mut connection: super::RegisteredConnection) {
    let initial = heartbeat_envelope(
        connection.node_id,
        connection.boot_id,
        Vec::new(),
        true,
        None,
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

    let mut cache = ResultCache::new();
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
                    Vec::new(),
                    true,
                    None,
                );
                if control.receive_envelope(connection.connection_id, envelope).await.is_err() {
                    break;
                }
            }
            command = connection.outbound.recv() => {
                let Some(command) = command else {
                    break;
                };
                if let Err(err) = handle_command(&control, &connection, &mut cache, command).await {
                    tracing::warn!(%err, node_id = %connection.node_id, "loopback node command failed");
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
    cache: &mut ResultCache,
    command: WireEnvelope,
) -> Result<(), NodeProtocolError> {
    if command.message_kind != "operation.request" {
        return Ok(());
    }
    let operation_id = command.operation_id.ok_or_else(|| {
        NodeProtocolError::InvalidRequest("operation request missing operation_id".into())
    })?;
    let result = match cache.get(operation_id) {
        Some(result) => result,
        None => {
            let request: OperationRequest = serde_json::from_value(command.payload)?;
            let result = execute_operation(request);
            cache.insert(operation_id, result.clone());
            result
        }
    };
    let envelope =
        operation_result_envelope(connection.node_id, connection.boot_id, operation_id, result)?;
    control
        .receive_envelope(connection.connection_id, envelope)
        .await
}

fn execute_operation(request: OperationRequest) -> OperationResultPayload {
    match request.kind.as_str() {
        "probe_echo" => OperationResultPayload {
            status: OperationResultStatus::Succeeded,
            result: Some(json!({ "echo": request.request })),
            error_code: None,
            error_message: None,
        },
        "reconcile_inventory" => OperationResultPayload {
            status: OperationResultStatus::Succeeded,
            result: Some(json!({ "live_session_ids": [] })),
            error_code: None,
            error_message: None,
        },
        _ => OperationResultPayload {
            status: OperationResultStatus::Failed,
            result: None,
            error_code: Some("unsupported_operation".into()),
            error_message: Some("operation is not supported by this node release".into()),
        },
    }
}
