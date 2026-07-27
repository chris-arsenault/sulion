pub mod client;
mod loopback;
pub mod model;
mod store;
mod transport;

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use chrono::{DateTime, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use uuid::Uuid;

pub use model::{
    CreateEnrollmentTokenRequest, DockerInfo, DockerPolicy, EnrollNodeRequest, EnrollNodeResponse,
    EnrollmentToken, NodeHello, NodeOperationKind, NodeOperationView, NodeRequestKind, NodeView,
    OperationResultPayload, OperationResultStatus, RequestResultPayload, TerminalBytesPayload,
    TerminalDeadPayload, TerminalResizePayload, WireEnvelope, CAPABILITY_OPERATION_PROBE,
    CAPABILITY_REPO_RUNTIME, CAPABILITY_SESSION_RECONCILE, CAPABILITY_SESSION_RUNTIME,
    CAPABILITY_TERMINAL_STREAM, CAPABILITY_WORKSPACE_RUNTIME, CONTROL_PROTOCOL_MAX,
    CONTROL_PROTOCOL_MIN, DEFAULT_HEARTBEAT_INTERVAL_SECS, DEFAULT_HEARTBEAT_TIMEOUT_SECS,
    MAX_NODE_FRAME_BYTES, NODE_PROTOCOL_VERSION, PATH_CONTRACT_VERSION,
};
pub use transport::{admin_router, public_router};

use model::{HelloAck, ReconciliationDirective};
use store::NodeStore;

const CONTROL_BUILD_GIT_SHA: &str = match option_env!("SULION_BUILD_GIT_SHA") {
    Some(value) => value,
    None => "dev",
};

#[derive(Debug, thiserror::Error)]
pub enum NodeProtocolError {
    #[error("node not found")]
    NotFound,
    #[error("unknown node")]
    UnknownNode,
    #[error("node credential is revoked")]
    Revoked,
    #[error("invalid or expired enrollment token")]
    InvalidEnrollmentToken,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("node authentication failed")]
    AuthenticationFailed,
    #[error("incompatible node: {0}")]
    Incompatible(String),
    #[error("node is unavailable")]
    Unavailable,
    #[error("node request failed ({code}): {message}")]
    Remote { code: String, message: String },
    #[error("idempotency key was reused with a different operation")]
    IdempotencyConflict,
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("cryptography: {0}")]
    Cryptography(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionAcceptance {
    SameBoot,
    NewBoot,
}

#[derive(Clone)]
pub struct NodeControl {
    store: NodeStore,
    active: Arc<RwLock<HashMap<Uuid, ActiveConnection>>>,
    request_waiters: Arc<RwLock<HashMap<Uuid, oneshot::Sender<RequestResultPayload>>>>,
    terminal_streams: Arc<RwLock<HashMap<Uuid, mpsc::Sender<TerminalEvent>>>>,
    heartbeat_interval_seconds: u64,
    heartbeat_timeout_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    Snapshot(Vec<u8>),
    Output(Vec<u8>),
    Ready,
    Dead(Option<i32>),
    Disconnected,
}

pub struct TerminalAttachment {
    sender: TerminalSender,
    pub events: mpsc::Receiver<TerminalEvent>,
}

#[derive(Clone)]
pub struct TerminalSender {
    control: Arc<NodeControl>,
    pub node_id: Uuid,
    pub session_id: Uuid,
    pub stream_id: Uuid,
}

impl TerminalAttachment {
    pub fn into_parts(self) -> (TerminalSender, mpsc::Receiver<TerminalEvent>) {
        (self.sender, self.events)
    }
}

impl TerminalSender {
    pub async fn send_input(&self, bytes: &[u8]) -> Result<(), NodeProtocolError> {
        let mut envelope = WireEnvelope::new(
            self.node_id,
            self.control.active_boot(self.node_id).await?,
            "terminal.input",
        );
        envelope.session_id = Some(self.session_id);
        envelope.stream_id = Some(self.stream_id);
        envelope.payload = serde_json::to_value(TerminalBytesPayload::from_bytes(bytes))?;
        self.control.send_envelope(self.node_id, envelope).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), NodeProtocolError> {
        let mut envelope = WireEnvelope::new(
            self.node_id,
            self.control.active_boot(self.node_id).await?,
            "terminal.resize",
        );
        envelope.session_id = Some(self.session_id);
        envelope.stream_id = Some(self.stream_id);
        envelope.payload = serde_json::to_value(TerminalResizePayload { cols, rows })?;
        self.control.send_envelope(self.node_id, envelope).await
    }

    pub async fn close(&self) {
        self.control
            .close_terminal(self.node_id, self.stream_id)
            .await;
    }
}

#[derive(Clone)]
struct ActiveConnection {
    connection_id: Uuid,
    boot_id: Uuid,
    outbound: mpsc::Sender<WireEnvelope>,
    cancel: watch::Sender<bool>,
}

pub(crate) struct RegisteredConnection {
    pub(crate) connection_id: Uuid,
    pub(crate) node_id: Uuid,
    pub(crate) boot_id: Uuid,
    pub(crate) ack: HelloAck,
    pub(crate) outbound: mpsc::Receiver<WireEnvelope>,
    pub(crate) canceled: watch::Receiver<bool>,
}

pub(crate) enum Registration {
    Accepted(RegisteredConnection),
    Rejected(HelloAck),
}

#[derive(Debug, Clone, Deserialize)]
struct HeartbeatPayload {
    #[serde(default)]
    live_session_ids: Vec<Uuid>,
    #[serde(default)]
    inventory_complete: bool,
    observed_release_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct OperationRequestPayload<'a> {
    kind: &'a str,
    resource_id: Option<Uuid>,
    request: &'a Value,
}

impl NodeControl {
    pub fn new(pool: crate::db::Pool) -> Arc<Self> {
        Self::with_heartbeat(
            pool,
            DEFAULT_HEARTBEAT_INTERVAL_SECS,
            DEFAULT_HEARTBEAT_TIMEOUT_SECS,
        )
    }

    pub fn with_heartbeat(
        pool: crate::db::Pool,
        heartbeat_interval_seconds: u64,
        heartbeat_timeout_seconds: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: NodeStore::new(pool, heartbeat_timeout_seconds),
            active: Arc::new(RwLock::new(HashMap::new())),
            request_waiters: Arc::new(RwLock::new(HashMap::new())),
            terminal_streams: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_interval_seconds,
            heartbeat_timeout_seconds,
        })
    }

    pub async fn create_enrollment_token(
        &self,
        display_name: &str,
        target_node_id: Option<Uuid>,
        ttl_seconds: Option<u64>,
    ) -> Result<EnrollmentToken, NodeProtocolError> {
        self.store
            .create_enrollment_token(display_name, target_node_id, ttl_seconds)
            .await
    }

    pub async fn enroll(
        &self,
        request: EnrollNodeRequest,
    ) -> Result<EnrollNodeResponse, NodeProtocolError> {
        let response = self.store.enroll(request).await?;
        self.cancel_active(response.node_id).await;
        Ok(response)
    }

    pub async fn revoke(&self, node_id: Uuid) -> Result<(), NodeProtocolError> {
        self.store.revoke(node_id).await?;
        self.cancel_active(node_id).await;
        Ok(())
    }

    pub async fn list_nodes(&self) -> Result<Vec<NodeView>, NodeProtocolError> {
        self.store.list_nodes().await
    }

    pub async fn request_operation(
        &self,
        node_id: Uuid,
        idempotency_key: &str,
        kind: NodeOperationKind,
        resource_id: Option<Uuid>,
        payload: Value,
    ) -> Result<NodeOperationView, NodeProtocolError> {
        let operation = self
            .store
            .request_operation(node_id, idempotency_key, kind, resource_id, payload)
            .await?;
        if matches!(
            operation.status.as_str(),
            "succeeded" | "failed" | "canceled"
        ) {
            return Ok(operation);
        }
        self.dispatch(&operation).await?;
        self.store
            .operation(operation.operation_id)
            .await?
            .ok_or(NodeProtocolError::NotFound)
    }

    pub async fn request_operation_and_wait(
        &self,
        node_id: Uuid,
        idempotency_key: &str,
        kind: NodeOperationKind,
        resource_id: Option<Uuid>,
        payload: Value,
    ) -> Result<Value, NodeProtocolError> {
        let operation = self
            .request_operation(node_id, idempotency_key, kind, resource_id, payload)
            .await?;
        let operation_id = operation.operation_id;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let operation = self
                .operation(operation_id)
                .await?
                .ok_or(NodeProtocolError::NotFound)?;
            match operation.status.as_str() {
                "succeeded" => return Ok(operation.result.unwrap_or(Value::Null)),
                "failed" | "canceled" => {
                    return Err(NodeProtocolError::Remote {
                        code: operation
                            .error_code
                            .unwrap_or_else(|| "operation_failed".into()),
                        message: operation
                            .error_message
                            .unwrap_or_else(|| "node operation failed".into()),
                    })
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(NodeProtocolError::Unavailable);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    pub async fn request(
        &self,
        node_id: Uuid,
        kind: NodeRequestKind,
        resource_id: Option<Uuid>,
        payload: Value,
    ) -> Result<Value, NodeProtocolError> {
        let boot_id = self.active_boot(node_id).await?;
        let request_id = Uuid::new_v4();
        let (response_tx, response_rx) = oneshot::channel();
        self.request_waiters
            .write()
            .await
            .insert(request_id, response_tx);
        let mut envelope = WireEnvelope::new(node_id, boot_id, "request");
        envelope.request_id = Some(request_id);
        envelope.payload = json!({
            "kind": kind.as_str(),
            "resource_id": resource_id,
            "request": payload,
        });
        if let Err(err) = self.send_envelope(node_id, envelope).await {
            self.request_waiters.write().await.remove(&request_id);
            return Err(err);
        }
        let response = tokio::time::timeout(std::time::Duration::from_secs(60), response_rx)
            .await
            .map_err(|_| NodeProtocolError::Unavailable)?
            .map_err(|_| NodeProtocolError::Unavailable)?;
        match response.status {
            OperationResultStatus::Succeeded => Ok(response.result.unwrap_or(Value::Null)),
            OperationResultStatus::Failed => Err(NodeProtocolError::Remote {
                code: response
                    .error_code
                    .unwrap_or_else(|| "request_failed".into()),
                message: response
                    .error_message
                    .unwrap_or_else(|| "node request failed".into()),
            }),
        }
    }

    pub async fn open_terminal(
        self: &Arc<Self>,
        node_id: Uuid,
        session_id: Uuid,
    ) -> Result<TerminalAttachment, NodeProtocolError> {
        let boot_id = self.active_boot(node_id).await?;
        let stream_id = Uuid::new_v4();
        let (events_tx, events_rx) = mpsc::channel(256);
        self.terminal_streams
            .write()
            .await
            .insert(stream_id, events_tx);
        let mut envelope = WireEnvelope::new(node_id, boot_id, "terminal.attach");
        envelope.session_id = Some(session_id);
        envelope.stream_id = Some(stream_id);
        if let Err(err) = self.send_envelope(node_id, envelope).await {
            self.terminal_streams.write().await.remove(&stream_id);
            return Err(err);
        }
        Ok(TerminalAttachment {
            sender: TerminalSender {
                control: self.clone(),
                node_id,
                session_id,
                stream_id,
            },
            events: events_rx,
        })
    }

    pub async fn first_available_node(&self) -> Result<Uuid, NodeProtocolError> {
        self.active
            .read()
            .await
            .keys()
            .copied()
            .min()
            .ok_or(NodeProtocolError::Unavailable)
    }

    pub async fn operation(
        &self,
        operation_id: Uuid,
    ) -> Result<Option<NodeOperationView>, NodeProtocolError> {
        self.store.operation(operation_id).await
    }

    pub async fn expire_heartbeats_at(
        &self,
        now: DateTime<Utc>,
    ) -> Result<usize, NodeProtocolError> {
        let expired = self.store.expire_heartbeats(now).await?;
        if expired.is_empty() {
            return Ok(0);
        }
        let mut active = self.active.write().await;
        for (node_id, _boot_id, connection_id) in &expired {
            if active
                .get(node_id)
                .is_some_and(|connection| connection.connection_id == *connection_id)
            {
                if let Some(connection) = active.remove(node_id) {
                    let _ = connection.cancel.send(true);
                }
            }
        }
        Ok(expired.len())
    }

    pub async fn run_heartbeat_monitor(self: Arc<Self>) {
        let cadence = std::time::Duration::from_secs(
            self.heartbeat_interval_seconds
                .clamp(1, self.heartbeat_timeout_seconds),
        );
        let mut interval = tokio::time::interval(cadence);
        loop {
            interval.tick().await;
            match self.expire_heartbeats_at(Utc::now()).await {
                Ok(count) if count > 0 => {
                    tracing::warn!(count, "development node heartbeat expired");
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "node heartbeat expiry check failed"),
            }
        }
    }

    pub async fn start_loopback(
        self: &Arc<Self>,
        node_id: Uuid,
        display_name: &str,
    ) -> Result<Uuid, NodeProtocolError> {
        loopback::start(self.clone(), node_id, display_name).await
    }

    pub async fn start_runtime_loopback(
        self: &Arc<Self>,
        runtime: Arc<crate::node_runtime::NodeRuntime>,
        display_name: &str,
    ) -> Result<Uuid, NodeProtocolError> {
        loopback::start_runtime(self.clone(), runtime, display_name).await
    }

    pub async fn active_connection_count(&self) -> usize {
        self.active.read().await.len()
    }

    pub(crate) fn challenge(&self) -> Result<model::ControlChallenge, NodeProtocolError> {
        Ok(model::ControlChallenge {
            challenge_id: Uuid::new_v4(),
            nonce: random_url_token(32)?,
            control_build_git_sha: CONTROL_BUILD_GIT_SHA.to_string(),
            control_protocol_min: CONTROL_PROTOCOL_MIN,
            control_protocol_max: CONTROL_PROTOCOL_MAX,
        })
    }

    pub(crate) async fn authenticate_and_register(
        &self,
        hello: NodeHello,
        challenge: &model::ControlChallenge,
    ) -> Result<Registration, NodeProtocolError> {
        validate_hello(&hello)?;
        let credential = self.store.credential(hello.node_id).await?;
        if credential.credential_kind != "ed25519" {
            return Err(NodeProtocolError::AuthenticationFailed);
        }
        let public_key = credential
            .public_key
            .ok_or(NodeProtocolError::AuthenticationFailed)?;
        let signature = hello.decode_signature().map_err(|err| {
            tracing::debug!(%err, node_id = %hello.node_id, "invalid node signature encoding");
            NodeProtocolError::AuthenticationFailed
        })?;
        signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
            .verify(&hello.signing_payload(challenge), &signature)
            .map_err(|_| NodeProtocolError::AuthenticationFailed)?;
        self.register_compatible(hello).await
    }

    pub(crate) async fn register_internal(
        &self,
        hello: NodeHello,
    ) -> Result<RegisteredConnection, NodeProtocolError> {
        validate_hello(&hello)?;
        match self.register_compatible(hello).await? {
            Registration::Accepted(connection) => Ok(connection),
            Registration::Rejected(ack) => Err(NodeProtocolError::Incompatible(
                ack.reason_code.unwrap_or_else(|| "unknown".into()),
            )),
        }
    }

    async fn register_compatible(
        &self,
        hello: NodeHello,
    ) -> Result<Registration, NodeProtocolError> {
        let accepted_capabilities = accepted_capabilities(&hello.capabilities);
        if let Err(reason) = compatibility_reason(&hello) {
            self.cancel_active(hello.node_id).await;
            self.store.record_incompatible(&hello, &reason).await?;
            return Ok(Registration::Rejected(self.hello_ack(
                false,
                Some(reason),
                accepted_capabilities,
                ReconciliationDirective::Reject,
            )));
        }
        let connection_id = Uuid::new_v4();
        let acceptance = self.store.record_connection(&hello, connection_id).await?;
        let directive = match acceptance {
            ConnectionAcceptance::SameBoot => ReconciliationDirective::Resume,
            ConnectionAcceptance::NewBoot => ReconciliationDirective::ReportInventory,
        };
        let (outbound_tx, outbound_rx) = mpsc::channel(128);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let active = ActiveConnection {
            connection_id,
            boot_id: hello.boot_id,
            outbound: outbound_tx,
            cancel: cancel_tx,
        };
        if let Some(previous) = self.active.write().await.insert(hello.node_id, active) {
            let _ = previous.cancel.send(true);
        }
        let registered = RegisteredConnection {
            connection_id,
            node_id: hello.node_id,
            boot_id: hello.boot_id,
            ack: self.hello_ack(true, None, accepted_capabilities, directive),
            outbound: outbound_rx,
            canceled: cancel_rx,
        };
        self.replay_pending(hello.node_id).await?;
        Ok(Registration::Accepted(registered))
    }

    fn hello_ack(
        &self,
        accepted: bool,
        reason_code: Option<String>,
        accepted_capabilities: Vec<String>,
        reconciliation: ReconciliationDirective,
    ) -> HelloAck {
        HelloAck {
            accepted,
            reason_code,
            control_build_git_sha: CONTROL_BUILD_GIT_SHA.to_string(),
            protocol_version: NODE_PROTOCOL_VERSION,
            accepted_capabilities,
            desired_release_digest: None,
            heartbeat_interval_secs: self.heartbeat_interval_seconds,
            heartbeat_timeout_secs: self.heartbeat_timeout_seconds,
            drain_state: "accepting".into(),
            reconciliation,
        }
    }

    async fn replay_pending(&self, node_id: Uuid) -> Result<(), NodeProtocolError> {
        for operation in self.store.pending_operations(node_id).await? {
            self.dispatch(&operation).await?;
        }
        Ok(())
    }

    async fn dispatch(&self, operation: &NodeOperationView) -> Result<(), NodeProtocolError> {
        let connection = self.active.read().await.get(&operation.node_id).cloned();
        let Some(connection) = connection else {
            return Ok(());
        };
        let Some(operation) = self
            .store
            .mark_dispatched(
                operation.operation_id,
                operation.node_id,
                connection.boot_id,
            )
            .await?
        else {
            return Ok(());
        };
        let mut envelope =
            WireEnvelope::new(operation.node_id, connection.boot_id, "operation.request");
        envelope.operation_id = Some(operation.operation_id);
        envelope.payload = serde_json::to_value(OperationRequestPayload {
            kind: &operation.kind,
            resource_id: operation.resource_id,
            request: &operation.request_payload,
        })?;
        connection
            .outbound
            .send(envelope)
            .await
            .map_err(|_| NodeProtocolError::Unavailable)
    }

    pub(crate) async fn receive_envelope(
        &self,
        connection_id: Uuid,
        envelope: WireEnvelope,
    ) -> Result<(), NodeProtocolError> {
        self.validate_active(connection_id, &envelope).await?;
        match envelope.message_kind.as_str() {
            "node.heartbeat" => {
                let payload: HeartbeatPayload = serde_json::from_value(envelope.payload)?;
                let current = self
                    .store
                    .heartbeat(
                        envelope.node_id,
                        envelope.boot_id,
                        connection_id,
                        &payload.live_session_ids,
                        payload.inventory_complete,
                        payload.observed_release_digest.as_deref(),
                    )
                    .await?;
                if !current {
                    return Err(NodeProtocolError::Unavailable);
                }
            }
            "operation.result" => {
                let operation_id = envelope.operation_id.ok_or_else(|| {
                    NodeProtocolError::InvalidRequest(
                        "operation.result requires operation_id".into(),
                    )
                })?;
                let payload: OperationResultPayload = serde_json::from_value(envelope.payload)?;
                self.store
                    .complete_operation(envelope.node_id, operation_id, &payload)
                    .await?;
            }
            "request.result" => {
                let request_id = envelope.request_id.ok_or_else(|| {
                    NodeProtocolError::InvalidRequest("request.result requires request_id".into())
                })?;
                let payload: RequestResultPayload = serde_json::from_value(envelope.payload)?;
                if let Some(waiter) = self.request_waiters.write().await.remove(&request_id) {
                    let _ = waiter.send(payload);
                }
            }
            "terminal.snapshot" | "terminal.output" | "terminal.ready" | "terminal.dead" => {
                self.receive_terminal_event(envelope).await?;
            }
            _ => {
                tracing::debug!(
                    kind = %envelope.message_kind,
                    node_id = %envelope.node_id,
                    "ignoring unknown compatible node message"
                );
            }
        }
        Ok(())
    }

    async fn validate_active(
        &self,
        connection_id: Uuid,
        envelope: &WireEnvelope,
    ) -> Result<(), NodeProtocolError> {
        if envelope.protocol_version != NODE_PROTOCOL_VERSION {
            return Err(NodeProtocolError::Incompatible(
                "envelope_protocol_version".into(),
            ));
        }
        let active = self.active.read().await;
        let Some(connection) = active.get(&envelope.node_id) else {
            return Err(NodeProtocolError::Unavailable);
        };
        if connection.connection_id != connection_id || connection.boot_id != envelope.boot_id {
            return Err(NodeProtocolError::AuthenticationFailed);
        }
        Ok(())
    }

    pub(crate) async fn disconnected(&self, node_id: Uuid, boot_id: Uuid, connection_id: Uuid) {
        let mut active = self.active.write().await;
        if active
            .get(&node_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            active.remove(&node_id);
        }
        drop(active);
        let streams = self
            .terminal_streams
            .read()
            .await
            .iter()
            .map(|(id, sender)| (*id, sender.clone()))
            .collect::<Vec<_>>();
        for (stream_id, sender) in streams {
            if sender.try_send(TerminalEvent::Disconnected).is_err() {
                self.terminal_streams.write().await.remove(&stream_id);
            }
        }
        if let Err(err) = self.store.disconnect(node_id, boot_id, connection_id).await {
            tracing::warn!(%err, %node_id, "failed to record node disconnect");
        }
    }

    async fn cancel_active(&self, node_id: Uuid) {
        if let Some(connection) = self.active.write().await.remove(&node_id) {
            let _ = connection.cancel.send(true);
        }
    }

    async fn active_boot(&self, node_id: Uuid) -> Result<Uuid, NodeProtocolError> {
        self.active
            .read()
            .await
            .get(&node_id)
            .map(|connection| connection.boot_id)
            .ok_or(NodeProtocolError::Unavailable)
    }

    async fn send_envelope(
        &self,
        node_id: Uuid,
        envelope: WireEnvelope,
    ) -> Result<(), NodeProtocolError> {
        let connection = self
            .active
            .read()
            .await
            .get(&node_id)
            .cloned()
            .ok_or(NodeProtocolError::Unavailable)?;
        connection
            .outbound
            .send(envelope)
            .await
            .map_err(|_| NodeProtocolError::Unavailable)
    }

    async fn close_terminal(&self, node_id: Uuid, stream_id: Uuid) {
        self.terminal_streams.write().await.remove(&stream_id);
        let Ok(boot_id) = self.active_boot(node_id).await else {
            return;
        };
        let mut envelope = WireEnvelope::new(node_id, boot_id, "terminal.detach");
        envelope.stream_id = Some(stream_id);
        let _ = self.send_envelope(node_id, envelope).await;
    }

    async fn receive_terminal_event(
        &self,
        envelope: WireEnvelope,
    ) -> Result<(), NodeProtocolError> {
        let stream_id = envelope.stream_id.ok_or_else(|| {
            NodeProtocolError::InvalidRequest("terminal event requires stream_id".into())
        })?;
        let event = match envelope.message_kind.as_str() {
            "terminal.snapshot" => TerminalEvent::Snapshot(
                serde_json::from_value::<TerminalBytesPayload>(envelope.payload)?
                    .into_bytes()
                    .map_err(|err| NodeProtocolError::InvalidRequest(err.to_string()))?,
            ),
            "terminal.output" => TerminalEvent::Output(
                serde_json::from_value::<TerminalBytesPayload>(envelope.payload)?
                    .into_bytes()
                    .map_err(|err| NodeProtocolError::InvalidRequest(err.to_string()))?,
            ),
            "terminal.ready" => TerminalEvent::Ready,
            "terminal.dead" => {
                let payload: TerminalDeadPayload = serde_json::from_value(envelope.payload)
                    .unwrap_or(TerminalDeadPayload { exit_code: None });
                TerminalEvent::Dead(payload.exit_code)
            }
            _ => return Ok(()),
        };
        let sender = self.terminal_streams.read().await.get(&stream_id).cloned();
        let Some(sender) = sender else {
            return Ok(());
        };
        if sender.try_send(event).is_err() {
            self.terminal_streams.write().await.remove(&stream_id);
            let Ok(boot_id) = self.active_boot(envelope.node_id).await else {
                return Ok(());
            };
            let mut detach = WireEnvelope::new(envelope.node_id, boot_id, "terminal.detach");
            detach.stream_id = Some(stream_id);
            let _ = self.send_envelope(envelope.node_id, detach).await;
        }
        Ok(())
    }
}

fn validate_hello(hello: &NodeHello) -> Result<(), NodeProtocolError> {
    if !valid_signed_text(&hello.build_git_sha, 128) {
        return Err(NodeProtocolError::InvalidRequest(
            "invalid build_git_sha".into(),
        ));
    }
    if hello.capabilities.len() > model::MAX_CAPABILITIES
        || hello
            .capabilities
            .iter()
            .any(|capability| !valid_protocol_identifier(capability, 100))
    {
        return Err(NodeProtocolError::InvalidRequest(
            "invalid capabilities".into(),
        ));
    }
    if hello
        .docker_info
        .server_version
        .as_deref()
        .is_some_and(|version| !valid_signed_text(version, 128))
        || hello
            .observed_release_digest
            .as_deref()
            .is_some_and(|digest| !valid_signed_text(digest, 256))
    {
        return Err(NodeProtocolError::InvalidRequest(
            "invalid signed handshake text".into(),
        ));
    }
    Ok(())
}

fn valid_signed_text(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_protocol_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
}

fn compatibility_reason(hello: &NodeHello) -> Result<(), String> {
    if !(CONTROL_PROTOCOL_MIN..=CONTROL_PROTOCOL_MAX).contains(&hello.protocol_version) {
        return Err("node_protocol_version".into());
    }
    if hello.supported_control_min > NODE_PROTOCOL_VERSION
        || hello.supported_control_max < NODE_PROTOCOL_VERSION
    {
        return Err("control_protocol_version".into());
    }
    if hello.path_contract_version != PATH_CONTRACT_VERSION {
        return Err("path_contract_version".into());
    }
    Ok(())
}

fn accepted_capabilities(declared: &[String]) -> Vec<String> {
    let supported = [
        CAPABILITY_OPERATION_PROBE,
        CAPABILITY_SESSION_RECONCILE,
        CAPABILITY_SESSION_RUNTIME,
        CAPABILITY_TERMINAL_STREAM,
        CAPABILITY_REPO_RUNTIME,
        CAPABILITY_WORKSPACE_RUNTIME,
    ];
    supported
        .into_iter()
        .filter(|capability| declared.iter().any(|declared| declared == capability))
        .map(str::to_string)
        .collect()
}

pub(crate) fn random_url_token(bytes: usize) -> Result<String, NodeProtocolError> {
    let mut value = vec![0_u8; bytes];
    SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| NodeProtocolError::Cryptography("secure random generation failed".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value))
}

pub fn operation_result_envelope(
    node_id: Uuid,
    boot_id: Uuid,
    operation_id: Uuid,
    result: OperationResultPayload,
) -> Result<WireEnvelope, NodeProtocolError> {
    let mut envelope = WireEnvelope::new(node_id, boot_id, "operation.result");
    envelope.operation_id = Some(operation_id);
    envelope.payload = serde_json::to_value(result)?;
    Ok(envelope)
}

pub fn heartbeat_envelope(
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: Vec<Uuid>,
    inventory_complete: bool,
    observed_release_digest: Option<String>,
) -> WireEnvelope {
    let mut envelope = WireEnvelope::new(node_id, boot_id, "node.heartbeat");
    envelope.payload = json!({
        "live_session_ids": live_session_ids,
        "inventory_complete": inventory_complete,
        "observed_release_digest": observed_release_digest,
    });
    envelope
}

#[cfg(test)]
mod tests {
    use super::{valid_protocol_identifier, valid_signed_text};

    #[test]
    fn signed_handshake_text_rejects_delimiter_injection() {
        assert!(valid_signed_text("sha256:abc", 128));
        assert!(!valid_signed_text("sha256:abc\nforged", 128));
        assert!(valid_protocol_identifier("operation.probe.v1", 100));
        assert!(!valid_protocol_identifier("operation,probe", 100));
    }
}
