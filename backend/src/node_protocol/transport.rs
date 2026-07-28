use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{post, Router};
use axum::Json;
use futures::StreamExt;
use uuid::Uuid;

use super::model::{
    ControlWireMessage, FragmentAssembler, NodeConfigPayload, NodeWireMessage, WireEnvelope,
    MAX_NODE_FRAME_BYTES, NODE_PROTOCOL_VERSION,
};
use super::NodeProtocolError;
use crate::AppState;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub fn public_router() -> Router<Arc<AppState>> {
    Router::new().route("/ws/nodes", axum::routing::get(connect))
}

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/nodes/:id/approve", post(approve_pairing))
}

async fn approve_pairing(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<Uuid>,
) -> Result<impl IntoResponse, NodeApiError> {
    state.node_control.approve_pairing(node_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect(
    ws: WebSocketUpgrade,
    peer: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    // Admission happens before the challenge, so an off-LAN caller learns
    // nothing about the handshake. It fails closed when the source cannot be
    // established rather than treating an unknown address as local.
    let forwarded = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok());
    let client = match state
        .node_control
        .source_policy()
        .admit(peer.map(|ConnectInfo(peer)| peer.ip()), forwarded)
    {
        Ok(client) => {
            tracing::debug!(%client, "node socket admitted");
            client
        }
        Err(refusal) => {
            tracing::warn!(%refusal, "node socket refused");
            return (StatusCode::FORBIDDEN, "node source address not permitted").into_response();
        }
    };
    ws.max_message_size(MAX_NODE_FRAME_BYTES)
        .max_frame_size(MAX_NODE_FRAME_BYTES)
        .on_upgrade(move |socket| node_socket(state, socket, client))
        .into_response()
}

async fn node_socket(state: Arc<AppState>, mut socket: WebSocket, client: std::net::IpAddr) {
    let challenge = match state.node_control.challenge() {
        Ok(challenge) => challenge,
        Err(err) => {
            tracing::warn!(%err, "failed to create node handshake challenge");
            return;
        }
    };
    if send_json(
        &mut socket,
        &ControlWireMessage::Challenge {
            challenge: challenge.clone(),
        },
    )
    .await
    .is_err()
    {
        return;
    }
    let hello = match receive_hello(&mut socket).await {
        Ok(hello) => hello,
        Err(err) => {
            tracing::warn!(%err, "development node handshake rejected");
            close_with_reason(&mut socket, 1008, "invalid node handshake").await;
            return;
        }
    };
    let node_id = hello.node_id;
    let connection = match state
        .node_control
        .authenticate_and_register(hello, &challenge)
        .await
    {
        Ok(registration) => registration,
        Err(NodeProtocolError::PairingRequired) => {
            tracing::info!(%node_id, "development node is awaiting approval");
            close_with_reason(&mut socket, 1008, "node approval required").await;
            return;
        }
        Err(err) => {
            tracing::warn!(%err, "development node authentication rejected");
            close_with_reason(&mut socket, 1008, "node authentication failed").await;
            return;
        }
    };
    run_connection(state, socket, connection, client).await;
}

async fn receive_hello(socket: &mut WebSocket) -> Result<super::NodeHello, NodeProtocolError> {
    let frame = tokio::time::timeout(HANDSHAKE_TIMEOUT, socket.next())
        .await
        .map_err(|_| NodeProtocolError::AuthenticationFailed)?
        .ok_or(NodeProtocolError::AuthenticationFailed)?
        .map_err(|_| NodeProtocolError::AuthenticationFailed)?;
    let Message::Text(text) = frame else {
        return Err(NodeProtocolError::AuthenticationFailed);
    };
    let message: NodeWireMessage = serde_json::from_str(&text)?;
    let NodeWireMessage::Hello { envelope, hello } = message else {
        return Err(NodeProtocolError::AuthenticationFailed);
    };
    if envelope.protocol_version != hello.protocol_version
        || envelope.node_id != hello.node_id
        || envelope.boot_id != hello.boot_id
        || envelope.message_kind != "node.hello"
    {
        return Err(NodeProtocolError::AuthenticationFailed);
    }
    Ok(hello)
}

async fn run_connection(
    state: Arc<AppState>,
    mut socket: WebSocket,
    mut connection: super::RegisteredConnection,
    client: std::net::IpAddr,
) {
    let ack = match ack_envelope(
        connection.ack.clone(),
        connection.node_id,
        connection.boot_id,
    ) {
        Ok(ack) => ack,
        Err(err) => {
            tracing::warn!(%err, "failed to encode node handshake response");
            return;
        }
    };
    if send_json(&mut socket, &ControlWireMessage::Envelope { envelope: ack })
        .await
        .is_err()
    {
        state
            .node_control
            .disconnected(
                connection.node_id,
                connection.boot_id,
                connection.connection_id,
            )
            .await;
        return;
    }

    if !deliver_node_config(&state, &connection, client, &mut socket).await {
        state
            .node_control
            .disconnected(
                connection.node_id,
                connection.boot_id,
                connection.connection_id,
            )
            .await;
        return;
    }

    let mut fragments = FragmentAssembler::default();
    loop {
        tokio::select! {
            changed = connection.canceled.changed() => {
                if changed.is_err() || *connection.canceled.borrow() {
                    close_gracefully(&mut socket, 1001, "connection superseded").await;
                    break;
                }
            }
            outbound = connection.outbound.recv() => {
                let Some(envelope) = outbound else {
                    close_gracefully(&mut socket, 1001, "connection superseded").await;
                    break;
                };
                if send_control_envelope(&mut socket, envelope).await.is_err() {
                    break;
                }
            }
            inbound = socket.next() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if receive_message(
                            &state,
                            connection.connection_id,
                            &mut fragments,
                            &text,
                        ).await.is_err() {
                            close_with_reason(&mut socket, 1008, "invalid node message").await;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    Some(Ok(Message::Binary(_))) => {
                        close_with_reason(&mut socket, 1003, "binary node frame unsupported").await;
                        break;
                    }
                }
            }
        }
    }
    state
        .node_control
        .disconnected(
            connection.node_id,
            connection.boot_id,
            connection.connection_id,
        )
        .await;
}

/// Hands an authenticated node the runtime configuration it would otherwise
/// need pre-provisioned. Returns whether the connection is still usable.
///
/// Always sends something, with an empty map when there is nothing to deliver,
/// so a node never waits out a timeout to learn that. Credentials are withheld
/// entirely from a connection that did not arrive over the tunnel.
async fn deliver_node_config(
    state: &Arc<AppState>,
    connection: &super::RegisteredConnection,
    client: std::net::IpAddr,
    socket: &mut WebSocket,
) -> bool {
    let may_deliver = state.node_control.may_deliver_credentials(Some(client));
    if !may_deliver {
        tracing::info!(
            node_id = %connection.node_id,
            %client,
            "withholding credentials until this node connects over the tunnel",
        );
    }
    let payload = match state
        .node_control
        .delivered_config()
        .filter(|_| may_deliver)
    {
        Some(config) => {
            tracing::info!(
                node_id = %connection.node_id,
                digest = %config.digest(),
                keys = ?config.key_names(),
                "delivering node runtime configuration",
            );
            let mut payload = config.payload();
            // Signed separately from the handshake and bound to this
            // connection's nonce, so the payload stays tamper-evident and
            // cannot be lifted onto another connection.
            payload.signature = state
                .node_control
                .identity()
                .map(|identity| identity.sign_config(&payload.digest, &connection.node_nonce));
            payload
        }
        None => NodeConfigPayload::empty(),
    };

    let mut envelope = WireEnvelope::new(
        connection.node_id,
        connection.boot_id,
        "control.node_config",
    );
    match serde_json::to_value(payload) {
        Ok(payload) => envelope.payload = payload,
        Err(err) => {
            tracing::error!(%err, "failed to encode node runtime configuration");
            return false;
        }
    }
    send_control_envelope(socket, envelope).await.is_ok()
}

async fn receive_message(
    state: &AppState,
    connection_id: Uuid,
    fragments: &mut FragmentAssembler,
    text: &str,
) -> Result<(), NodeProtocolError> {
    let message: NodeWireMessage = serde_json::from_str(text)?;
    let NodeWireMessage::Envelope { envelope } = message else {
        return Err(NodeProtocolError::AuthenticationFailed);
    };
    let envelope = fragments
        .push(envelope)
        .map_err(|error| NodeProtocolError::InvalidRequest(error.to_string()))?;
    match envelope {
        Some(envelope) => {
            state
                .node_control
                .receive_envelope(connection_id, envelope)
                .await
        }
        None => Ok(()),
    }
}

fn ack_envelope(
    ack: super::model::HelloAck,
    node_id: Uuid,
    boot_id: Uuid,
) -> Result<WireEnvelope, NodeProtocolError> {
    let mut envelope = WireEnvelope::new(node_id, boot_id, "control.hello_ack");
    envelope.protocol_version = NODE_PROTOCOL_VERSION;
    envelope.payload = serde_json::to_value(ack)?;
    Ok(envelope)
}

async fn send_json<T: serde::Serialize>(socket: &mut WebSocket, value: &T) -> Result<(), ()> {
    let text = serde_json::to_string(value).map_err(|_| ())?;
    socket.send(Message::Text(text)).await.map_err(|_| ())
}

async fn send_control_envelope(socket: &mut WebSocket, envelope: WireEnvelope) -> Result<(), ()> {
    let fragments = super::model::fragment_envelope(&envelope).map_err(|_| ())?;
    for envelope in fragments {
        send_json(socket, &ControlWireMessage::Envelope { envelope }).await?;
    }
    Ok(())
}

async fn close_with_reason(socket: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn close_gracefully(socket: &mut WebSocket, code: u16, reason: &'static str) {
    close_with_reason(socket, code, reason).await;
    let _ = tokio::time::timeout(Duration::from_millis(250), socket.next()).await;
}

struct NodeApiError(NodeProtocolError);

impl From<NodeProtocolError> for NodeApiError {
    fn from(value: NodeProtocolError) -> Self {
        Self(value)
    }
}

impl IntoResponse for NodeApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self.0 {
            NodeProtocolError::NotFound | NodeProtocolError::UnknownNode => {
                (StatusCode::NOT_FOUND, "node_not_found")
            }
            NodeProtocolError::PairingRequired => (StatusCode::CONFLICT, "node_pairing_required"),
            NodeProtocolError::AuthenticationFailed => {
                (StatusCode::UNAUTHORIZED, "node_authentication_failed")
            }
            NodeProtocolError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            NodeProtocolError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "node_unavailable"),
            NodeProtocolError::Remote { .. } => (StatusCode::BAD_GATEWAY, "node_request_failed"),
            NodeProtocolError::Database(_)
            | NodeProtocolError::Serialization(_)
            | NodeProtocolError::Cryptography(_) => {
                tracing::error!(error = %self.0, "node protocol request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
        };
        (status, Json(serde_json::json!({ "error": code }))).into_response()
    }
}
