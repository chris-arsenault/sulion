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

use super::config::NodeRuntimeConfig;
use super::model::{
    ControlChallenge, ControlWireMessage, FragmentAssembler, HelloAck, NodeConfigPayload,
    NodeWireMessage, RequestResultPayload, TerminalBytesPayload, TerminalResizePayload,
};
use super::pin::{ControlPin, PinOutcome};
use super::{
    heartbeat_envelope, NodeHello, NodeRequestKind, RequestResultStatus, WireEnvelope,
    NODE_PROTOCOL_VERSION,
};
use crate::node_runtime::{NodeRuntime, SessionInputRequest, SessionResizeRequest};

const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);
/// Cadence for re-attempting enrollment. Long enough that an unapproved node
/// waiting on a human is not a busy loop, short enough that clicking approve
/// feels immediate.
const ENROLL_BACKOFF: Duration = Duration::from_secs(5);
/// How long to wait for `control.node_config` after the acknowledgment before
/// concluding this deployment delivers no configuration.
const CONFIG_WAIT: Duration = Duration::from_secs(10);

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

/// Result of one enrollment handshake.
enum EnrollOutcome {
    Delivered(NodeConfigPayload),
    /// Authenticated, but this control plane forwards no configuration. The
    /// node is expected to have been configured out of band.
    NoConfiguration,
    /// Control closed the handshake, with its stated reason. Normally this is
    /// "node approval required" and the node is waiting on a human; anything
    /// else means the peer would not accept this node at all.
    Rejected(String),
}

/// Obtains runtime configuration over the authenticated channel before the node
/// has any credentials of its own.
///
/// This is the whole bootstrap: the node proves possession of its identity key,
/// an operator approves the fingerprint once in the UI, and the configuration
/// that would otherwise have been copied onto the machine by hand arrives here.
/// Blocks until approval, so a freshly installed enclave simply waits.
pub async fn await_runtime_config(
    config: &NodeClientConfig,
    key: &Ed25519KeyPair,
    boot_id: Uuid,
) -> Option<NodeRuntimeConfig> {
    let mut announced: Option<String> = None;
    loop {
        match enroll_once(config, key, boot_id).await {
            Ok(EnrollOutcome::Delivered(payload)) => match NodeRuntimeConfig::accept(payload) {
                Ok(delivered) => return Some(delivered),
                Err(error) => {
                    // Never write a payload this node did not expect; treat
                    // it as a hostile or misconfigured peer and retry.
                    tracing::error!(%error, "rejected delivered node configuration");
                }
            },
            Ok(EnrollOutcome::NoConfiguration) => {
                tracing::info!(
                    "control plane delivers no node configuration; using the local environment"
                );
                return None;
            }
            Ok(EnrollOutcome::Rejected(reason)) => {
                // Logged once per distinct reason: this loop runs until a human
                // acts or the other end is upgraded, and repeating either
                // message every few seconds would bury everything else.
                if announced.as_deref() != Some(reason.as_str()) {
                    if reason == "node approval required" {
                        tracing::info!(
                            node_id = %config.node_id,
                            "awaiting operator approval; approve this node in the Sulion \
                             stats panel",
                        );
                    } else {
                        tracing::warn!(
                            node_id = %config.node_id,
                            %reason,
                            "control plane refused this node's handshake; retrying. If the \
                             control plane is still on an older release this clears once it \
                             is deployed",
                        );
                    }
                    announced = Some(reason);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "node enrollment attempt failed; retrying");
            }
        }
        tokio::time::sleep(ENROLL_BACKOFF).await;
    }
}

/// Connects to control, over pinned TLS for `wss://` URLs. Returns the socket
/// and, for TLS connections, the DER of the certificate the server presented.
async fn connect_control(
    url: &str,
) -> anyhow::Result<(
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Option<Vec<u8>>,
)> {
    if url.starts_with("wss://") {
        let pin = super::tls::TlsPin::from_env();
        let (verifier, seen) = super::tls::PinnedServerVerifier::new(pin.pinned());
        let connector =
            tokio_tungstenite::Connector::Rustls(Arc::new(super::tls::client_config(verifier)?));
        let (socket, _) =
            tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector))
                .await?;
        let seen = seen.lock().expect("seen certificate slot").clone();
        Ok((socket, seen))
    } else {
        let (socket, _) = tokio_tungstenite::connect_async(url).await?;
        Ok((socket, None))
    }
}

/// Confirms the TLS certificate this connection actually used is the one the
/// control identity signed for, and pins it on first pairing.
///
/// This is what makes first TLS contact safe once the Ed25519 identity is
/// known, and what makes every later contact refuse a substituted certificate
/// even before the pin file exists.
fn verify_tls_binding(
    seen_cert: Option<&[u8]>,
    proof: Option<&super::model::ControlHelloProof>,
    can_persist: bool,
) -> anyhow::Result<()> {
    let Some(seen) = seen_cert else {
        // Plain ws://: permitted only where the client already allows an
        // insecure URL (host-local development and tests).
        return Ok(());
    };
    let seen_digest = super::tls::cert_digest(seen);
    match proof.and_then(|proof| proof.tls_cert_digest.as_deref()) {
        Some(signed) if signed == seen_digest => {}
        Some(_) => anyhow::bail!(
            "the TLS certificate presented does not match the digest the control \
             identity signed; refusing to continue"
        ),
        None if proof.is_some() => anyhow::bail!(
            "control proved its identity but did not bind the TLS certificate; \
             refusing to continue"
        ),
        None => tracing::warn!(
            "TLS connection with no identity proof; certificate is trust-on-first-use only"
        ),
    }
    let pin = super::tls::TlsPin::from_env();
    if pin.pinned().is_none() && can_persist {
        pin.record(seen)?;
        tracing::info!(
            digest = %seen_digest,
            path = %pin.path().display(),
            "pinned the control plane's TLS certificate",
        );
    }
    Ok(())
}

async fn enroll_once(
    config: &NodeClientConfig,
    key: &Ed25519KeyPair,
    boot_id: Uuid,
) -> anyhow::Result<EnrollOutcome> {
    let (socket, seen_cert) = connect_control(&config.control_url).await?;
    let (mut sink, mut source) = socket.split();
    let challenge = receive_challenge(&mut source).await?;
    if challenge.protocol_version != NODE_PROTOCOL_VERSION {
        anyhow::bail!("control and node protocol versions differ");
    }
    let node_nonce = super::random_url_token(32)?;
    let hello = signed_hello(config.node_id, boot_id, key, &challenge, node_nonce.clone());
    let mut hello_envelope = WireEnvelope::new(config.node_id, boot_id, "node.hello");
    hello_envelope.protocol_version = hello.protocol_version;
    send_node_message(
        &mut sink,
        NodeWireMessage::Hello {
            envelope: hello_envelope,
            hello: hello.clone(),
        },
    )
    .await?;

    // An unapproved node is closed out right after its hello, which is the
    // normal first-boot path rather than an error.
    let acknowledgment = match receive_ack(&mut source).await {
        Ok(acknowledgment) => {
            if acknowledgment.protocol_version != NODE_PROTOCOL_VERSION {
                anyhow::bail!("control acknowledged an unexpected protocol version");
            }
            acknowledgment
        }
        Err(error) => {
            if let Some(closed) = error.downcast_ref::<ControlClosed>() {
                return Ok(EnrollOutcome::Rejected(closed.0.clone()));
            }
            return Err(error);
        }
    };

    // Everything past this point is only safe if this really is the control
    // plane that paired the node, so establish that before reading any of it.
    let pin = ControlPin::from_env();
    match pin.verify(acknowledgment.control_proof.as_ref(), &challenge, &hello)? {
        PinOutcome::Matched => {}
        PinOutcome::FirstPairing(public_key) => {
            pin.record(&public_key)?;
            tracing::info!(
                control_key = %public_key,
                path = %pin.path().display(),
                "pinned the control plane that paired this node",
            );
        }
        PinOutcome::Unauthenticated => {
            tracing::warn!("control plane offered no identity; connection is unauthenticated");
        }
    }
    // The identity is established; now bind the encrypted transport to it.
    // Enrollment runs as root, so this is also where the certificate pin and
    // its runtime copy are written.
    verify_tls_binding(
        seen_cert.as_deref(),
        acknowledgment.control_proof.as_ref(),
        true,
    )?;
    let mut fragments = FragmentAssembler::default();
    let deadline = tokio::time::Instant::now() + CONFIG_WAIT;
    loop {
        let frame = match tokio::time::timeout_at(deadline, source.next()).await {
            Err(_) => return Ok(EnrollOutcome::NoConfiguration),
            Ok(None) => return Ok(EnrollOutcome::Rejected("connection closed".into())),
            Ok(Some(frame)) => frame?,
        };
        let Message::Text(text) = frame else {
            continue;
        };
        let ControlWireMessage::Envelope { envelope } = serde_json::from_str(&text)? else {
            anyhow::bail!("unexpected control challenge after authentication");
        };
        let Some(envelope) = fragments.push(envelope)? else {
            continue;
        };
        if envelope.message_kind == "control.node_config" {
            let payload: NodeConfigPayload = serde_json::from_value(envelope.payload)?;
            if payload.is_empty() {
                return Ok(EnrollOutcome::NoConfiguration);
            }
            // The handshake authenticated the peer; this authenticates the
            // payload itself, which matters while the channel is not encrypted.
            pin.verify_config(&payload.digest, &node_nonce, payload.signature.as_deref())?;
            return Ok(EnrollOutcome::Delivered(payload));
        }
    }
}

/// The control plane hung up during the handshake. Carries the reason so the
/// node can say whether it is waiting on a human or being rejected, which are
/// very different things to be looking at in a log.
#[derive(Debug, thiserror::Error)]
#[error("control closed the node connection: {0}")]
struct ControlClosed(String);

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
    let (socket, seen_cert) = connect_control(&config.control_url).await?;
    let (mut sink, mut source) = socket.split();
    let challenge = receive_challenge(&mut source).await?;
    if challenge.protocol_version != NODE_PROTOCOL_VERSION {
        anyhow::bail!("control and node protocol versions differ");
    }
    let node_nonce = super::random_url_token(32)?;
    let hello = signed_hello(
        runtime.node_id(),
        runtime.boot_id(),
        &key,
        &challenge,
        node_nonce.clone(),
    );
    let mut hello_envelope = WireEnvelope::new(runtime.node_id(), runtime.boot_id(), "node.hello");
    hello_envelope.protocol_version = hello.protocol_version;
    send_node_message(
        &mut sink,
        NodeWireMessage::Hello {
            envelope: hello_envelope,
            hello: hello.clone(),
        },
    )
    .await?;
    let acknowledgment = receive_ack(&mut source).await?;
    if acknowledgment.protocol_version != NODE_PROTOCOL_VERSION {
        anyhow::bail!("control acknowledged an unexpected protocol version");
    }
    // Same identity and transport checks as enrollment; this process has
    // dropped privileges, so it verifies but never writes pins.
    let pin = ControlPin::from_env();
    pin.verify(acknowledgment.control_proof.as_ref(), &challenge, &hello)?;
    verify_tls_binding(
        seen_cert.as_deref(),
        acknowledgment.control_proof.as_ref(),
        false,
    )?;
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
        "control.node_config" => {
            // Delivered again on every connection. By this point the process has
            // dropped to the unprivileged runtime identity and can no longer
            // write root-owned node state, so a rotation is reported and picked
            // up by the enrollment stage on the next start.
            let payload: NodeConfigPayload = serde_json::from_value(command.payload)?;
            if payload.is_empty() {
                return Ok(());
            }
            let delivered = NodeRuntimeConfig::accept(payload)?;
            if !delivered.matches_current_env() {
                tracing::warn!(
                    digest = %delivered.digest(),
                    keys = ?delivered.key_names(),
                    "control plane delivered new node configuration; \
                     it is applied when sulion-node next starts",
                );
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
    node_id: Uuid,
    boot_id: Uuid,
    key: &Ed25519KeyPair,
    challenge: &ControlChallenge,
    node_nonce: String,
) -> NodeHello {
    let mut hello = NodeHello {
        node_id,
        boot_id,
        node_nonce,
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
        .ok_or_else(|| ControlClosed("connection closed".into()))??;
    let Message::Text(text) = frame else {
        // A close here is the control plane declining to acknowledge: either
        // this node is not approved yet, or it was rejected outright.
        if let Message::Close(frame) = &frame {
            let reason = frame
                .as_ref()
                .map(|frame| frame.reason.to_string())
                .unwrap_or_else(|| "no reason given".into());
            return Err(ControlClosed(reason).into());
        }
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
