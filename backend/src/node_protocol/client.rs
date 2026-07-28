//! Outbound development-node client.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use super::model::{
    ControlChallenge, ControlWireMessage, FragmentAssembler, HelloAck, NodeWireMessage,
    RequestResultPayload, TerminalBytesPayload, TerminalResizePayload,
};
use super::{
    heartbeat_envelope, NodeHello, NodeRequestKind, RequestResultStatus, WireEnvelope,
    NODE_PROTOCOL_VERSION,
};
use crate::node_runtime::{NodeRuntime, SessionInputRequest, SessionResizeRequest};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct NodeClientConfig {
    pub control_url: String,
    pub node_id: Uuid,
    pub private_key_path: PathBuf,
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
        Ok(Self {
            control_url,
            node_id,
            private_key_path,
        })
    }
}

#[derive(Debug, Deserialize)]
struct EphemeralRequest {
    kind: String,
    request: Value,
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
    loop {
        match connect_once(&config, runtime.clone(), key.clone()).await {
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
) -> anyhow::Result<()> {
    let (socket, _) = tokio_tungstenite::connect_async(&config.control_url).await?;
    let (mut sink, mut source) = socket.split();
    let challenge = receive_challenge(&mut source).await?;
    if challenge.protocol_version != NODE_PROTOCOL_VERSION {
        anyhow::bail!("control and node protocol versions differ");
    }
    let hello = signed_hello(&runtime, &key, &challenge);
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
    if acknowledgment.protocol_version != NODE_PROTOCOL_VERSION {
        anyhow::bail!("control acknowledged an unexpected protocol version");
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
                        tokio::spawn(async move {
                            if let Err(error) =
                                handle_command(runtime, outbound, envelope).await
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
    command: WireEnvelope,
) -> anyhow::Result<()> {
    match command.message_kind.as_str() {
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
    runtime: &NodeRuntime,
    key: &Ed25519KeyPair,
    challenge: &ControlChallenge,
) -> NodeHello {
    let mut hello = NodeHello {
        node_id: runtime.node_id(),
        boot_id: runtime.boot_id(),
        protocol_version: NODE_PROTOCOL_VERSION,
        public_key: Some(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.public_key().as_ref()),
        ),
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

fn unsupported_request() -> RequestResultPayload {
    RequestResultPayload {
        status: RequestResultStatus::Failed,
        result: None,
        error_code: Some("unsupported_request".into()),
        error_message: Some("request is not supported by this node release".into()),
    }
}

fn ensure_request_succeeded(result: RequestResultPayload) -> anyhow::Result<()> {
    match result.status {
        RequestResultStatus::Succeeded => Ok(()),
        RequestResultStatus::Failed => anyhow::bail!(
            "{}: {}",
            result.error_code.unwrap_or_else(|| "request_failed".into()),
            result
                .error_message
                .unwrap_or_else(|| "node request failed".into())
        ),
    }
}

pub fn generate_private_key(path: &Path) -> anyhow::Result<()> {
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
    Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|_| anyhow::anyhow!("generated key could not be parsed"))?;
    Ok(())
}
