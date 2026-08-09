pub mod client;
mod commands;
pub mod config;
pub mod gateway;
pub mod identity;
mod lan;
mod loopback;
pub mod model;
pub mod pin;
mod store;
pub mod tls;
mod transport;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base64::Engine;
use chrono::{DateTime, Utc};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use uuid::Uuid;

pub use config::NodeRuntimeConfig;
pub use identity::ControlIdentity;
pub use lan::{NodeLanGuard, NodeSourcePolicy, SourceRefusal};
pub use model::{
    ControlHelloProof, NodeConfigPayload, NodeDevenvStatus, NodeHello, NodeHostStats,
    NodeRequestKind, NodeView, RequestResultPayload, RequestResultStatus, TerminalBytesPayload,
    TerminalDeadPayload, TerminalResizePayload, WireEnvelope, DEDICATED_NODE_ID,
    DEFAULT_HEARTBEAT_INTERVAL_SECS, DEFAULT_HEARTBEAT_TIMEOUT_SECS, DEFAULT_NODE_LAN_CIDR,
    MAX_NODE_FRAME_BYTES, NODE_PROTOCOL_VERSION,
};
pub use pin::{ControlPin, PinOutcome};
pub use transport::{admin_router, public_router};

pub const MULTI_REPO_SESSION_CAPABILITY: &str = "multi_repo_session_v1";

use model::HelloAck;
use store::NodeStore;

#[derive(Debug, thiserror::Error)]
pub enum NodeProtocolError {
    #[error("node not found")]
    NotFound,
    #[error("unknown node")]
    UnknownNode,
    #[error("node is awaiting operator approval")]
    PairingRequired,
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("node authentication failed")]
    AuthenticationFailed,
    #[error("node is unavailable")]
    Unavailable,
    #[error("node request failed ({code}): {message}")]
    Remote { code: String, message: String },
    #[error("database: {0}")]
    Database(#[from] sqlx::Error),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("cryptography: {0}")]
    Cryptography(String),
}

pub struct NodeControl {
    store: NodeStore,
    active: Arc<RwLock<HashMap<Uuid, ActiveConnection>>>,
    request_waiters: Arc<RwLock<HashMap<Uuid, oneshot::Sender<RequestResultPayload>>>>,
    terminal_streams: Arc<RwLock<HashMap<Uuid, mpsc::Sender<TerminalEvent>>>>,
    /// Latest whole-machine sample from each connected node. Current pressure
    /// only: it is dropped on disconnect rather than aged, because a stale
    /// figure from a node that is gone is worse than no figure.
    host_stats: Arc<RwLock<HashMap<Uuid, NodeHostStats>>>,
    /// Latest devenv link report from each connected node, same lifecycle as
    /// `host_stats`: current only, dropped on disconnect.
    devenv_status: Arc<RwLock<HashMap<Uuid, NodeDevenvStatus>>>,
    /// Features reported by the currently connected node. An absent heartbeat
    /// field means an older node with no additive capabilities.
    capabilities: Arc<RwLock<HashMap<Uuid, HashSet<String>>>>,
    heartbeat_interval_seconds: u64,
    heartbeat_timeout_seconds: u64,
    /// Runtime configuration handed to a node once it authenticates. Unset
    /// leaves nodes to their own environment, which is how the portable
    /// standalone deployment and the integration suite run.
    delivered_config: std::sync::OnceLock<NodeRuntimeConfig>,
    /// Source-address boundary for inbound node sockets. Unset admits every
    /// source; the control plane applies its policy during startup.
    source_policy: std::sync::OnceLock<NodeSourcePolicy>,
    /// Signing identity a node pins on first pairing. Unset on the loopback
    /// and test paths, which have no network peer to impersonate.
    identity: std::sync::OnceLock<ControlIdentity>,
    /// Digest of the TLS certificate served on the node port, signed into the
    /// handshake proof so the transport is bound to the approved identity.
    tls_cert_digest: std::sync::OnceLock<String>,
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
    /// Bound into the configuration signature so a payload cannot be replayed
    /// onto a different connection.
    pub(crate) node_nonce: String,
    pub(crate) ack: HelloAck,
    pub(crate) outbound: mpsc::Receiver<WireEnvelope>,
    pub(crate) canceled: watch::Receiver<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct HeartbeatPayload {
    #[serde(default)]
    live_session_ids: Vec<Uuid>,
    #[serde(default)]
    inventory_complete: bool,
    #[serde(default)]
    host: Option<NodeHostStats>,
    #[serde(default)]
    devenv: Option<NodeDevenvStatus>,
    #[serde(default)]
    capabilities: Vec<String>,
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
            host_stats: Arc::new(RwLock::new(HashMap::new())),
            devenv_status: Arc::new(RwLock::new(HashMap::new())),
            capabilities: Arc::new(RwLock::new(HashMap::new())),
            heartbeat_interval_seconds,
            heartbeat_timeout_seconds,
            delivered_config: std::sync::OnceLock::new(),
            source_policy: std::sync::OnceLock::new(),
            identity: std::sync::OnceLock::new(),
            tls_cert_digest: std::sync::OnceLock::new(),
        })
    }

    pub async fn approve_pairing(&self, node_id: Uuid) -> Result<(), NodeProtocolError> {
        self.store.approve_pairing(node_id).await?;
        self.cancel_active(node_id).await;
        Ok(())
    }

    pub async fn list_nodes(&self) -> Result<Vec<NodeView>, NodeProtocolError> {
        self.store.list_nodes().await
    }

    pub async fn request(
        &self,
        node_id: Uuid,
        kind: NodeRequestKind,
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
            RequestResultStatus::Succeeded => Ok(response.result.unwrap_or(Value::Null)),
            RequestResultStatus::Failed => Err(NodeProtocolError::Remote {
                code: response
                    .error_code
                    .unwrap_or_else(|| "request_failed".into()),
                message: response
                    .error_message
                    .unwrap_or_else(|| "node request failed".into()),
            }),
        }
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

    pub async fn is_connected(&self, node_id: Uuid) -> bool {
        self.active.read().await.contains_key(&node_id)
    }

    pub async fn supports_capability(&self, node_id: Uuid, capability: &str) -> bool {
        self.capabilities
            .read()
            .await
            .get(&node_id)
            .is_some_and(|reported| reported.contains(capability))
    }

    pub(crate) fn challenge(&self) -> Result<model::ControlChallenge, NodeProtocolError> {
        Ok(model::ControlChallenge {
            challenge_id: Uuid::new_v4(),
            nonce: random_url_token(32)?,
            protocol_version: NODE_PROTOCOL_VERSION,
        })
    }

    pub(crate) async fn authenticate_and_register(
        &self,
        hello: NodeHello,
        challenge: &model::ControlChallenge,
    ) -> Result<RegisteredConnection, NodeProtocolError> {
        validate_hello(&hello)?;
        let credential = self.store.credential(hello.node_id).await?;
        if credential
            .as_ref()
            .is_some_and(|credential| credential.credential_kind != "ed25519")
            || hello.protocol_version != challenge.protocol_version
        {
            return Err(NodeProtocolError::AuthenticationFailed);
        }
        let offered_public_key = hello
            .public_key
            .as_deref()
            .map(decode_node_public_key)
            .transpose()?;
        let public_key = offered_public_key
            .as_deref()
            .or_else(|| {
                credential
                    .as_ref()
                    .and_then(|credential| credential.public_key.as_deref())
            })
            .ok_or(NodeProtocolError::AuthenticationFailed)?;
        let signature = hello.decode_signature().map_err(|err| {
            tracing::debug!(%err, node_id = %hello.node_id, "invalid node signature encoding");
            NodeProtocolError::AuthenticationFailed
        })?;
        signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
            .verify(&hello.signing_payload(challenge), &signature)
            .map_err(|_| NodeProtocolError::AuthenticationFailed)?;

        if let Some(offered_public_key) = offered_public_key {
            let approved = credential
                .as_ref()
                .and_then(|credential| credential.public_key.as_deref());
            if approved != Some(offered_public_key.as_slice()) {
                self.store
                    .request_pairing(hello.node_id, &offered_public_key)
                    .await?;
                return Err(NodeProtocolError::PairingRequired);
            }
        }

        // Proving control's identity is what lets an already-paired node refuse
        // anything else that answers on this address.
        let proof = self.identity().map(|identity| {
            identity.prove_handshake(
                challenge,
                &hello,
                self.tls_cert_digest.get().map(String::as_str),
            )
        });
        self.register(hello, proof).await
    }

    pub(crate) async fn register_internal(
        &self,
        hello: NodeHello,
    ) -> Result<RegisteredConnection, NodeProtocolError> {
        validate_hello(&hello)?;
        self.register(hello, None).await
    }

    async fn register(
        &self,
        hello: NodeHello,
        proof: Option<ControlHelloProof>,
    ) -> Result<RegisteredConnection, NodeProtocolError> {
        if hello.protocol_version != NODE_PROTOCOL_VERSION {
            return Err(NodeProtocolError::AuthenticationFailed);
        }
        let connection_id = Uuid::new_v4();
        self.store.record_connection(&hello, connection_id).await?;
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
            ack: self.hello_ack(proof),
            node_nonce: hello.node_nonce.clone(),
            outbound: outbound_rx,
            canceled: cancel_rx,
        };
        Ok(registered)
    }

    /// Builds the acknowledgment, signing it when this control plane has an
    /// identity.
    fn hello_ack(&self, proof: Option<ControlHelloProof>) -> HelloAck {
        HelloAck {
            protocol_version: NODE_PROTOCOL_VERSION,
            heartbeat_interval_secs: self.heartbeat_interval_seconds,
            control_proof: proof,
        }
    }

    pub(crate) fn identity(&self) -> Option<&ControlIdentity> {
        self.identity.get()
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
                    )
                    .await?;
                if !current {
                    return Err(NodeProtocolError::Unavailable);
                }
                if let Some(host) = payload.host {
                    self.host_stats.write().await.insert(envelope.node_id, host);
                }
                if let Some(devenv) = payload.devenv {
                    self.devenv_status
                        .write()
                        .await
                        .insert(envelope.node_id, devenv);
                }
                self.capabilities
                    .write()
                    .await
                    .insert(envelope.node_id, payload.capabilities.into_iter().collect());
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
            return Err(NodeProtocolError::AuthenticationFailed);
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
            self.host_stats.write().await.remove(&node_id);
            self.devenv_status.write().await.remove(&node_id);
            self.capabilities.write().await.remove(&node_id);
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
            self.host_stats.write().await.remove(&node_id);
            self.devenv_status.write().await.remove(&node_id);
            self.capabilities.write().await.remove(&node_id);
            let _ = connection.cancel.send(true);
        }
    }

    /// Latest machine sample from the connected node, or None while no node
    /// has reported one. The deployment supports one node, so the first
    /// active connection carrying a sample is the answer.
    pub async fn host_stats(&self) -> Option<NodeHostStats> {
        let active = self.active.read().await;
        let stats = self.host_stats.read().await;
        active
            .keys()
            .find_map(|node_id| stats.get(node_id).copied())
    }

    /// Latest devenv link report from the connected node, or None while no
    /// node has reported one (a node from a release without the field never
    /// will). Same single-node convention as `host_stats`.
    pub async fn devenv_status(&self) -> Option<NodeDevenvStatus> {
        let active = self.active.read().await;
        let status = self.devenv_status.read().await;
        active
            .keys()
            .find_map(|node_id| status.get(node_id).cloned())
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
}

/// Liveness: expiring connections whose node stopped heartbeating.
impl NodeControl {
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
                    self.capabilities.write().await.remove(node_id);
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
}

/// Terminal stream plumbing between browser attachers and the owning node.
impl NodeControl {
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

/// Deployment policy: what this control plane proves about itself, what it
/// is willing to hand a node, and where a node may connect from.
impl NodeControl {
    /// Applies the deployment's node admission and delivery policy.
    ///
    /// Called once during control-plane startup. Every other construction path
    /// (tests, loopback, standalone) leaves the policy unset, which admits any
    /// source and delivers no configuration.
    pub fn apply_policy(
        &self,
        delivered_config: Option<NodeRuntimeConfig>,
        source_policy: NodeSourcePolicy,
        identity: Option<ControlIdentity>,
        tls_cert_digest: Option<String>,
    ) -> anyhow::Result<()> {
        if let Some(delivered_config) = delivered_config {
            self.delivered_config
                .set(delivered_config)
                .map_err(|_| anyhow::anyhow!("node runtime configuration is already applied"))?;
        }
        if let Some(identity) = identity {
            self.identity
                .set(identity)
                .map_err(|_| anyhow::anyhow!("control identity is already applied"))?;
        }
        if let Some(tls_cert_digest) = tls_cert_digest {
            self.tls_cert_digest
                .set(tls_cert_digest)
                .map_err(|_| anyhow::anyhow!("control TLS digest is already applied"))?;
        }
        self.source_policy
            .set(source_policy)
            .map_err(|_| anyhow::anyhow!("node source boundary is already applied"))
    }

    pub(crate) fn source_policy(&self) -> &NodeSourcePolicy {
        self.source_policy
            .get()
            .unwrap_or(&lan::UNRESTRICTED_POLICY)
    }

    pub(crate) fn delivered_config(&self) -> Option<&NodeRuntimeConfig> {
        self.delivered_config.get()
    }
}

fn validate_hello(hello: &NodeHello) -> Result<(), NodeProtocolError> {
    if hello.protocol_version != NODE_PROTOCOL_VERSION {
        return Err(NodeProtocolError::AuthenticationFailed);
    }
    Ok(())
}

fn decode_node_public_key(value: &str) -> Result<Vec<u8>, NodeProtocolError> {
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| NodeProtocolError::AuthenticationFailed)?;
    if decoded.len() != 32 {
        return Err(NodeProtocolError::AuthenticationFailed);
    }
    Ok(decoded)
}

pub(crate) fn random_url_token(bytes: usize) -> Result<String, NodeProtocolError> {
    let mut value = vec![0_u8; bytes];
    SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| NodeProtocolError::Cryptography("secure random generation failed".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value))
}

pub fn heartbeat_envelope(
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: Vec<Uuid>,
    inventory_complete: bool,
    host: Option<NodeHostStats>,
    devenv: Option<NodeDevenvStatus>,
) -> WireEnvelope {
    let mut envelope = WireEnvelope::new(node_id, boot_id, "node.heartbeat");
    envelope.payload = json!({
        "live_session_ids": live_session_ids,
        "inventory_complete": inventory_complete,
        "host": host,
        "devenv": devenv,
    });
    envelope
}

pub fn runtime_heartbeat_envelope(
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: Vec<Uuid>,
    inventory_complete: bool,
    host: Option<NodeHostStats>,
    devenv: Option<NodeDevenvStatus>,
) -> WireEnvelope {
    let mut envelope = heartbeat_envelope(
        node_id,
        boot_id,
        live_session_ids,
        inventory_complete,
        host,
        devenv,
    );
    if let Some(payload) = envelope.payload.as_object_mut() {
        payload.insert(
            "capabilities".into(),
            json!([MULTI_REPO_SESSION_CAPABILITY]),
        );
    }
    envelope
}

#[cfg(test)]
mod tests {
    use super::{random_url_token, HeartbeatPayload};

    #[test]
    fn random_tokens_are_nonempty_and_distinct() {
        let first = random_url_token(32).unwrap();
        let second = random_url_token(32).unwrap();
        assert!(!first.is_empty());
        assert_ne!(first, second);
    }

    /// Control and node deploy on their own schedules, so a payload must
    /// survive a peer that predates a field and a peer that adds one. This is
    /// what makes deployment order irrelevant, and it is why adding a field
    /// never justifies a protocol version bump.
    #[test]
    fn a_heartbeat_survives_a_peer_from_another_release() {
        let older: HeartbeatPayload = serde_json::from_value(serde_json::json!({
            "live_session_ids": [],
            "inventory_complete": true,
        }))
        .expect("a heartbeat without the newer field still parses");
        assert!(older.host.is_none());
        assert!(older.devenv.is_none());
        assert!(older.capabilities.is_empty());

        let newer: HeartbeatPayload = serde_json::from_value(serde_json::json!({
            "live_session_ids": [],
            "inventory_complete": true,
            "host": {"memory_used_bytes": 1, "memory_total_bytes": 2, "cpu_percent": 3.0},
            "something_this_build_has_never_heard_of": 1,
        }))
        .expect("a heartbeat with unknown fields still parses");
        assert_eq!(newer.host.map(|host| host.memory_total_bytes), Some(2));
    }
}
