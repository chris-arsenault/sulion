use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{post, Router};
use axum::Json;
use futures::StreamExt;
use uuid::Uuid;

use super::model::{
    ControlWireMessage, CreateEnrollmentTokenRequest, EnrollNodeRequest, FragmentAssembler,
    NodeWireMessage, WireEnvelope, MAX_NODE_FRAME_BYTES, NODE_PROTOCOL_VERSION,
};
use super::{NodeProtocolError, Registration};
use crate::AppState;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub fn public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/nodes/enroll", post(enroll))
        .route("/ws/nodes", axum::routing::get(connect))
}

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/nodes/enrollment-tokens",
            post(create_enrollment_token),
        )
        .route("/api/nodes/:id/revoke", post(revoke))
}

async fn create_enrollment_token(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateEnrollmentTokenRequest>,
) -> Result<impl IntoResponse, NodeApiError> {
    let token = state
        .node_control
        .create_enrollment_token(
            &request.display_name,
            request.target_node_id,
            request.ttl_seconds,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(token)))
}

async fn enroll(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EnrollNodeRequest>,
) -> Result<impl IntoResponse, NodeApiError> {
    let enrolled = state.node_control.enroll(request).await?;
    Ok((StatusCode::CREATED, Json(enrolled)))
}

async fn revoke(
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<Uuid>,
) -> Result<StatusCode, NodeApiError> {
    state.node_control.revoke(node_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn connect(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.max_message_size(MAX_NODE_FRAME_BYTES)
        .max_frame_size(MAX_NODE_FRAME_BYTES)
        .on_upgrade(move |socket| node_socket(state, socket))
}

async fn node_socket(state: Arc<AppState>, mut socket: WebSocket) {
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
    let hello_node_id = hello.node_id;
    let hello_boot_id = hello.boot_id;
    let registration = match state
        .node_control
        .authenticate_and_register(hello, &challenge)
        .await
    {
        Ok(registration) => registration,
        Err(err) => {
            tracing::warn!(%err, "development node authentication rejected");
            close_with_reason(&mut socket, 1008, "node authentication failed").await;
            return;
        }
    };
    match registration {
        Registration::Rejected(ack) => {
            if let Ok(envelope) =
                ack_envelope(ack, challenge.challenge_id, hello_node_id, hello_boot_id)
            {
                let _ = send_json(&mut socket, &ControlWireMessage::Envelope { envelope }).await;
            }
            close_with_reason(&mut socket, 1008, "incompatible node").await;
        }
        Registration::Accepted(connection) => run_connection(state, socket, connection).await,
    }
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
) {
    let ack = match ack_envelope(
        connection.ack.clone(),
        Uuid::new_v4(),
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
    request_id: Uuid,
    node_id: Uuid,
    boot_id: Uuid,
) -> Result<WireEnvelope, NodeProtocolError> {
    let mut envelope = WireEnvelope::new(node_id, boot_id, "control.hello_ack");
    envelope.protocol_version = NODE_PROTOCOL_VERSION;
    envelope.request_id = Some(request_id);
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
            NodeProtocolError::InvalidEnrollmentToken => {
                (StatusCode::UNAUTHORIZED, "invalid_enrollment_token")
            }
            NodeProtocolError::Revoked | NodeProtocolError::AuthenticationFailed => {
                (StatusCode::UNAUTHORIZED, "node_authentication_failed")
            }
            NodeProtocolError::InvalidRequest(_) => (StatusCode::BAD_REQUEST, "invalid_request"),
            NodeProtocolError::Incompatible(_) => (StatusCode::CONFLICT, "node_incompatible"),
            NodeProtocolError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "node_unavailable"),
            NodeProtocolError::IdempotencyConflict => {
                (StatusCode::CONFLICT, "idempotency_conflict")
            }
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
