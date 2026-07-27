//! Outbound development-node client.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use super::model::{
    ControlChallenge, ControlWireMessage, FragmentAssembler, HelloAck, NodeWireMessage,
    RequestResultPayload, TerminalBytesPayload, TerminalResizePayload,
};
use super::{
    heartbeat_envelope, operation_result_envelope, DockerInfo, DockerPolicy, NodeHello,
    NodeOperationKind, NodeRequestKind, OperationResultPayload, OperationResultStatus,
    WireEnvelope, CAPABILITY_OPERATION_PROBE, CAPABILITY_REPO_RUNTIME,
    CAPABILITY_SESSION_RECONCILE, CAPABILITY_SESSION_RUNTIME, CAPABILITY_TERMINAL_STREAM,
    CAPABILITY_WORKSPACE_RUNTIME, CONTROL_PROTOCOL_MAX, CONTROL_PROTOCOL_MIN,
    NODE_PROTOCOL_VERSION, PATH_CONTRACT_VERSION,
};
use crate::node_runtime::{NodeRuntime, SessionInputRequest, SessionResizeRequest};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);
const RESULT_CACHE_CAPACITY: usize = 512;

#[derive(Debug, Clone)]
pub struct NodeClientConfig {
    pub control_url: String,
    pub node_id: Uuid,
    pub private_key_path: PathBuf,
    pub build_git_sha: String,
    pub observed_release_digest: Option<String>,
    pub docker_policy: DockerPolicy,
    pub docker_info: DockerInfo,
}

impl NodeClientConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let control_url = std::env::var("SULION_NODE_CONTROL_URL")
            .map_err(|_| anyhow::anyhow!("SULION_NODE_CONTROL_URL must be set"))?;
        let insecure_ws_allowed = std::env::var("SULION_NODE_ALLOW_INSECURE_WS")
            .is_ok_and(|value| value == "1" || value == "true");
        if !control_url.starts_with("wss://")
            && !(control_url.starts_with("ws://")
                && (cfg!(debug_assertions) || insecure_ws_allowed))
        {
            anyhow::bail!(
                "SULION_NODE_CONTROL_URL must use wss://; set SULION_NODE_ALLOW_INSECURE_WS=1 only for a host-local connection"
            );
        }
        let node_id = std::env::var("SULION_NODE_ID")
            .map_err(|_| anyhow::anyhow!("SULION_NODE_ID must be set"))?
            .parse()
            .map_err(|_| anyhow::anyhow!("SULION_NODE_ID must be a UUID"))?;
        let private_key_path = PathBuf::from(
            std::env::var("SULION_NODE_PRIVATE_KEY_PATH")
                .map_err(|_| anyhow::anyhow!("SULION_NODE_PRIVATE_KEY_PATH must be set"))?,
        );
        let docker_policy = match std::env::var("SULION_DOCKER_MODE")
            .unwrap_or_else(|_| "direct".into())
            .as_str()
        {
            "direct" => DockerPolicy::Direct,
            "brokered" => DockerPolicy::Brokered,
            "none" => DockerPolicy::None,
            _ => anyhow::bail!("SULION_DOCKER_MODE must be direct, brokered, or none"),
        };
        Ok(Self {
            control_url,
            node_id,
            private_key_path,
            build_git_sha: option_env!("SULION_BUILD_GIT_SHA")
                .or(option_env!("GITHUB_SHA"))
                .unwrap_or("dev")
                .to_string(),
            observed_release_digest: std::env::var("SULION_RELEASE_DIGEST").ok(),
            docker_policy,
            docker_info: DockerInfo {
                server_version: std::env::var("SULION_DOCKER_SERVER_VERSION").ok(),
                rootless: std::env::var("SULION_DOCKER_ROOTLESS")
                    .map(|value| value != "0" && value != "false")
                    .unwrap_or(docker_policy == DockerPolicy::Direct),
            },
        })
    }
}

#[derive(Debug, Deserialize)]
struct OperationRequest {
    kind: String,
    request: Value,
}

#[derive(Debug, Deserialize)]
struct EphemeralRequest {
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
            if let Some(operation_id) = self.order.pop_front() {
                self.values.remove(&operation_id);
            }
        }
    }
}

pub async fn run(config: NodeClientConfig, runtime: Arc<NodeRuntime>) -> anyhow::Result<()> {
    let key = Arc::new(load_private_key(&config.private_key_path)?);
    run_with_key(config, runtime, key).await
}

pub async fn run_with_key(
    config: NodeClientConfig,
    runtime: Arc<NodeRuntime>,
    key: Arc<Ed25519KeyPair>,
) -> anyhow::Result<()> {
    if config.node_id != runtime.node_id() {
        anyhow::bail!("node client identity does not match runtime identity");
    }
    let cache = Arc::new(Mutex::new(ResultCache::new()));
    loop {
        match connect_once(&config, runtime.clone(), key.clone(), cache.clone()).await {
            Ok(()) => tracing::warn!("node connection closed; reconnecting"),
            Err(error) => tracing::warn!(%error, "node connection failed; reconnecting"),
        }
        tokio::time::sleep(RECONNECT_BACKOFF).await;
    }
}

async fn connect_once(
    config: &NodeClientConfig,
    runtime: Arc<NodeRuntime>,
    key: Arc<Ed25519KeyPair>,
    cache: Arc<Mutex<ResultCache>>,
) -> anyhow::Result<()> {
    let (socket, _) = tokio_tungstenite::connect_async(&config.control_url).await?;
    let (mut sink, mut source) = socket.split();
    let challenge = receive_challenge(&mut source).await?;
    let hello = signed_hello(config, &runtime, &key, &challenge);
    let mut hello_envelope = WireEnvelope::new(runtime.node_id(), runtime.boot_id(), "node.hello");
    hello_envelope.protocol_version = hello.protocol_version;
    send_node_message(
        &mut sink,
        NodeWireMessage::Hello {
            envelope: hello_envelope,
            hello,
        },
    )
    .await?;
    let acknowledgment = receive_ack(&mut source).await?;
    if !acknowledgment.accepted {
        anyhow::bail!(
            "control rejected node: {}",
            acknowledgment
                .reason_code
                .unwrap_or_else(|| "unknown_reason".into())
        );
    }
    tracing::info!(
        node_id = %runtime.node_id(),
        boot_id = %runtime.boot_id(),
        "development node connected",
    );

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<WireEnvelope>(512);
    let mut fragments = FragmentAssembler::default();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(
        acknowledgment.heartbeat_interval_secs.max(1),
    ));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let heartbeat = heartbeat_envelope(
                    runtime.node_id(),
                    runtime.boot_id(),
                    runtime.live_session_ids().await,
                    true,
                    config.observed_release_digest.clone(),
                );
                send_node_envelope(&mut sink, heartbeat).await?;
            }
            outbound = outbound_rx.recv() => {
                let Some(envelope) = outbound else {
                    anyhow::bail!("node outbound channel closed");
                };
                send_node_envelope(&mut sink, envelope).await?;
            }
            inbound = source.next() => {
                let frame = inbound
                    .ok_or_else(|| anyhow::anyhow!("control websocket closed"))??;
                match frame {
                    Message::Text(text) => {
                        let message: ControlWireMessage = serde_json::from_str(&text)?;
                        let ControlWireMessage::Envelope { envelope } = message else {
                            anyhow::bail!("unexpected control challenge after authentication");
                        };
                        let Some(envelope) = fragments.push(envelope)? else {
                            continue;
                        };
                        let runtime = runtime.clone();
                        let outbound = outbound_tx.clone();
                        let cache = cache.clone();
                        tokio::spawn(async move {
                            if let Err(error) =
                                handle_command(runtime, outbound, cache, envelope).await
                            {
                                tracing::warn!(%error, "node command failed");
                            }
                        });
                    }
                    Message::Ping(payload) => sink.send(Message::Pong(payload)).await?,
                    Message::Pong(_) => {}
                    Message::Close(frame) => {
                        anyhow::bail!("control closed node websocket: {frame:?}");
                    }
                    Message::Binary(_) => anyhow::bail!("binary control frame is unsupported"),
                    Message::Frame(_) => {}
                }
            }
        }
    }
}

async fn handle_command(
    runtime: Arc<NodeRuntime>,
    outbound: mpsc::Sender<WireEnvelope>,
    cache: Arc<Mutex<ResultCache>>,
    command: WireEnvelope,
) -> anyhow::Result<()> {
    match command.message_kind.as_str() {
        "operation.request" => {
            let operation_id = command
                .operation_id
                .ok_or_else(|| anyhow::anyhow!("operation missing operation_id"))?;
            let cached = cache.lock().await.get(operation_id);
            let result = match cached {
                Some(result) => result,
                None => {
                    let request: OperationRequest = serde_json::from_value(command.payload)?;
                    let result = match NodeOperationKind::parse(&request.kind) {
                        Some(kind) => runtime.execute_operation(kind, request.request).await,
                        None => unsupported_operation(),
                    };
                    cache.lock().await.insert(operation_id, result.clone());
                    result
                }
            };
            outbound
                .send(operation_result_envelope(
                    runtime.node_id(),
                    runtime.boot_id(),
                    operation_id,
                    result,
                )?)
                .await?;
        }
        "request" => {
            let request_id = command
                .request_id
                .ok_or_else(|| anyhow::anyhow!("request missing request_id"))?;
            let request: EphemeralRequest = serde_json::from_value(command.payload)?;
            let result = match NodeRequestKind::parse(&request.kind) {
                Some(kind) => runtime.execute_request(kind, request.request).await,
                None => unsupported_request(),
            };
            let mut envelope =
                WireEnvelope::new(runtime.node_id(), runtime.boot_id(), "request.result");
            envelope.request_id = Some(request_id);
            envelope.payload = serde_json::to_value(result)?;
            outbound.send(envelope).await?;
        }
        "terminal.attach" => {
            runtime
                .open_terminal(
                    command
                        .stream_id
                        .ok_or_else(|| anyhow::anyhow!("terminal attach missing stream_id"))?,
                    command
                        .session_id
                        .ok_or_else(|| anyhow::anyhow!("terminal attach missing session_id"))?,
                    outbound,
                )
                .await?;
        }
        "terminal.detach" => {
            if let Some(stream_id) = command.stream_id {
                runtime.close_terminal(stream_id).await;
            }
        }
        "terminal.input" => {
            let session_id = command
                .session_id
                .ok_or_else(|| anyhow::anyhow!("terminal input missing session_id"))?;
            let bytes =
                serde_json::from_value::<TerminalBytesPayload>(command.payload)?.into_bytes()?;
            ensure_request_succeeded(
                runtime
                    .execute_request(
                        NodeRequestKind::SessionInput,
                        serde_json::to_value(SessionInputRequest::from_bytes(session_id, &bytes))?,
                    )
                    .await,
            )?;
        }
        "terminal.resize" => {
            let session_id = command
                .session_id
                .ok_or_else(|| anyhow::anyhow!("terminal resize missing session_id"))?;
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
            )?;
        }
        _ => {
            tracing::debug!(kind = %command.message_kind, "ignoring unknown control message");
        }
    }
    Ok(())
}

fn signed_hello(
    config: &NodeClientConfig,
    runtime: &NodeRuntime,
    key: &Ed25519KeyPair,
    challenge: &ControlChallenge,
) -> NodeHello {
    let mut hello = NodeHello {
        node_id: runtime.node_id(),
        boot_id: runtime.boot_id(),
        build_git_sha: config.build_git_sha.clone(),
        protocol_version: NODE_PROTOCOL_VERSION,
        supported_control_min: CONTROL_PROTOCOL_MIN,
        supported_control_max: CONTROL_PROTOCOL_MAX,
        capabilities: vec![
            CAPABILITY_OPERATION_PROBE.into(),
            CAPABILITY_SESSION_RECONCILE.into(),
            CAPABILITY_SESSION_RUNTIME.into(),
            CAPABILITY_TERMINAL_STREAM.into(),
            CAPABILITY_REPO_RUNTIME.into(),
            CAPABILITY_WORKSPACE_RUNTIME.into(),
        ],
        docker_policy: config.docker_policy,
        docker_info: config.docker_info.clone(),
        path_contract_version: PATH_CONTRACT_VERSION,
        observed_release_digest: config.observed_release_digest.clone(),
        signature: String::new(),
    };
    hello.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(key.sign(&hello.signing_payload(challenge)).as_ref());
    hello
}

async fn receive_challenge<S>(source: &mut S) -> anyhow::Result<ControlChallenge>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let frame = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .map_err(|_| anyhow::anyhow!("control challenge timed out"))?
        .ok_or_else(|| anyhow::anyhow!("control closed before challenge"))??;
    let Message::Text(text) = frame else {
        anyhow::bail!("control challenge must be text");
    };
    match serde_json::from_str::<ControlWireMessage>(&text)? {
        ControlWireMessage::Challenge { challenge } => Ok(challenge),
        _ => anyhow::bail!("expected control challenge"),
    }
}

async fn receive_ack<S>(source: &mut S) -> anyhow::Result<HelloAck>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let frame = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .map_err(|_| anyhow::anyhow!("control acknowledgment timed out"))?
        .ok_or_else(|| anyhow::anyhow!("control closed before acknowledgment"))??;
    let Message::Text(text) = frame else {
        anyhow::bail!("control acknowledgment must be text");
    };
    let ControlWireMessage::Envelope { envelope } =
        serde_json::from_str::<ControlWireMessage>(&text)?
    else {
        anyhow::bail!("expected control acknowledgment");
    };
    if envelope.message_kind != "control.hello_ack" {
        anyhow::bail!("unexpected control message before acknowledgment");
    }
    Ok(serde_json::from_value(envelope.payload)?)
}

async fn send_node_message<S>(sink: &mut S, message: NodeWireMessage) -> anyhow::Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = serde_json::to_string(&message)?;
    sink.send(Message::Text(text)).await?;
    Ok(())
}

async fn send_node_envelope<S>(sink: &mut S, envelope: WireEnvelope) -> anyhow::Result<()>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    for envelope in super::model::fragment_envelope(&envelope)? {
        send_node_message(sink, NodeWireMessage::Envelope { envelope }).await?;
    }
    Ok(())
}

pub fn load_private_key(path: &Path) -> anyhow::Result<Ed25519KeyPair> {
    let bytes = std::fs::read(path)
        .map_err(|error| anyhow::anyhow!("read node private key {}: {error}", path.display()))?;
    Ed25519KeyPair::from_pkcs8(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid Ed25519 PKCS#8 key at {}", path.display()))
}

fn unsupported_operation() -> OperationResultPayload {
    OperationResultPayload {
        status: OperationResultStatus::Failed,
        result: None,
        error_code: Some("unsupported_operation".into()),
        error_message: Some("operation is not supported by this node release".into()),
    }
}

fn unsupported_request() -> RequestResultPayload {
    RequestResultPayload {
        status: OperationResultStatus::Failed,
        result: None,
        error_code: Some("unsupported_request".into()),
        error_message: Some("request is not supported by this node release".into()),
    }
}

fn ensure_request_succeeded(result: RequestResultPayload) -> anyhow::Result<()> {
    match result.status {
        OperationResultStatus::Succeeded => Ok(()),
        OperationResultStatus::Failed => anyhow::bail!(
            "{}: {}",
            result.error_code.unwrap_or_else(|| "request_failed".into()),
            result
                .error_message
                .unwrap_or_else(|| "node request failed".into())
        ),
    }
}

pub fn generate_private_key(path: &Path) -> anyhow::Result<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .map_err(|_| anyhow::anyhow!("Ed25519 key generation failed"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(document.as_ref())?;
    let key = Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|_| anyhow::anyhow!("generated key could not be parsed"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.public_key().as_ref()))
}

pub async fn enroll(
    control_http_url: &str,
    token: &str,
    private_key_path: &Path,
) -> anyhow::Result<super::EnrollNodeResponse> {
    let key = load_private_key(private_key_path)?;
    let public_key =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/nodes/enroll",
            control_http_url.trim_end_matches('/')
        ))
        .json(&json!({"token": token, "public_key": public_key}))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "node enrollment failed with HTTP {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }
    Ok(response.json().await?)
}
