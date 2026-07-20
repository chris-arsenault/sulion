//! WebSocket attach route. Clients connect to `/ws/sessions/:id`, receive
//! the current shadow-emulator snapshot as the first binary frame, then
//! live PTY bytes as subsequent binary frames. Input and resize commands
//! are sent by the client as JSON text frames.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use futures::{sink::SinkExt, stream::StreamExt};
use portable_pty::PtySize;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast::error::RecvError, mpsc, Mutex};
use uuid::Uuid;

use super::routes::{ApiError, ApiResult};
use crate::auth::AuthenticatedUser;
use crate::pty::PtySession;
use crate::AppState;

const WS_PROTOCOL: &str = "sulion.v1";
const WS_TICKET_PROTOCOL_PREFIX: &str = "sulion.ticket.";
const WS_TICKET_TTL: Duration = Duration::from_secs(30);

struct WsTicket {
    session_id: Uuid,
    expires_at: Instant,
}

#[derive(Default)]
pub struct WsTicketStore {
    tickets: Mutex<HashMap<String, WsTicket>>,
}

impl WsTicketStore {
    async fn issue(&self, session_id: Uuid) -> anyhow::Result<String> {
        self.issue_for(session_id, WS_TICKET_TTL).await
    }

    async fn issue_for(&self, session_id: Uuid, ttl: Duration) -> anyhow::Result<String> {
        let mut raw = [0_u8; 32];
        SystemRandom::new()
            .fill(&mut raw)
            .map_err(|_| anyhow::anyhow!("failed to generate websocket ticket"))?;
        let ticket = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
        let key = hash_ticket(&ticket);
        let now = Instant::now();
        let mut tickets = self.tickets.lock().await;
        tickets.retain(|_, record| record.expires_at > now);
        tickets.insert(
            key,
            WsTicket {
                session_id,
                expires_at: now + ttl,
            },
        );
        Ok(ticket)
    }

    async fn consume(&self, ticket: &str, session_id: Uuid) -> bool {
        let key = hash_ticket(ticket);
        let now = Instant::now();
        let mut tickets = self.tickets.lock().await;
        tickets.retain(|_, record| record.expires_at > now);
        matches!(tickets.remove(&key), Some(record) if record.session_id == session_id && record.expires_at > now)
    }
}

fn hash_ticket(ticket: &str) -> String {
    let hash = digest::digest(&digest::SHA256, ticket.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.as_ref())
}

#[derive(Deserialize)]
pub struct IssueTicketRequest {
    session_id: Uuid,
}

#[derive(Serialize)]
pub struct IssueTicketResponse {
    ticket: String,
    expires_in: u64,
}

pub async fn issue_ticket(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthenticatedUser>,
    Json(body): Json<IssueTicketRequest>,
) -> ApiResult<Json<IssueTicketResponse>> {
    if state.pty.get(body.session_id).await.is_none() {
        return Err(ApiError::NotFound);
    }
    let ticket = state
        .ws_tickets
        .issue(body.session_id)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(IssueTicketResponse {
        ticket,
        expires_in: WS_TICKET_TTL.as_secs(),
    }))
}

/// Client → server control messages. Wrapped as tagged JSON for
/// forward-compatibility.
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ClientMsg {
    /// Keystroke data. `data` is a UTF-8 string (xterm.js's onData format).
    Input { data: String },
    /// Resize event.
    Resize { cols: u16, rows: u16 },
    /// Client-initiated ping. Useful when intermediaries eat low-level
    /// WebSocket pings.
    Ping,
}

/// Server → client status envelope. Sent as JSON text frames.
#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum ServerMsg {
    /// Marks the end of the initial snapshot — useful diagnostic.
    Ready,
    /// Application-level heartbeat response for browser clients.
    Pong,
    /// PTY has exited. `exit` is the captured status if known.
    Dead { exit: Option<i32> },
}

pub async fn attach(
    Path(id): Path<Uuid>,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(ticket) = ticket_from_protocols(&headers) else {
        return unauthorized();
    };
    if !state.ws_tickets.consume(&ticket, id).await {
        return unauthorized();
    }
    let Some(session) = state.pty.get(id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "no such session").into_response();
    };
    let ws_test_hooks = state.ws_test_hooks.clone();
    ws.protocols([WS_PROTOCOL])
        .on_upgrade(move |socket| handle_socket(socket, session, ws_test_hooks))
}

fn ticket_from_protocols(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .find_map(|protocol| {
            protocol
                .strip_prefix(WS_TICKET_PROTOCOL_PREFIX)
                .filter(|ticket| !ticket.is_empty())
                .map(str::to_string)
        })
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

async fn handle_socket(
    socket: WebSocket,
    session: Arc<PtySession>,
    ws_test_hooks: Arc<crate::WsTestHooks>,
) {
    let (mut tx, mut rx) = socket.split();

    // Snapshot first — gives the client the current TUI state.
    let snapshot = session.emulator.snapshot();
    if !snapshot.is_empty() && tx.send(Message::Binary(snapshot)).await.is_err() {
        return;
    }
    let _ = tx
        .send(Message::Text(
            serde_json::to_string(&ServerMsg::Ready).unwrap(),
        ))
        .await;

    let mut out_rx = session.output.subscribe();
    let input = session.input.clone();
    let resize = session.resize.clone();
    let mut drop_ws_rx = ws_test_hooks.subscribe(session.id).await;
    let (control_tx, mut control_rx) = mpsc::channel::<ServerMsg>(8);

    // Outbound task: broadcast → WS
    let outbound_session_id = session.id;
    let outbound = tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = out_rx.recv() => {
                    match msg {
                        Ok(bytes) => {
                            if tx.send(Message::Binary(bytes)).await.is_err() {
                                break;
                            }
                        }
                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!(session = %outbound_session_id, lagged = n, "broadcast lagged");
                            continue;
                        }
                        Err(RecvError::Closed) => {
                            // PTY EOF — tell the client and close cleanly.
                            let _ = tx
                                .send(Message::Text(
                                    serde_json::to_string(&ServerMsg::Dead { exit: None }).unwrap(),
                                ))
                                .await;
                            let _ = tx.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
                _ = drop_ws_rx.recv() => {
                    let _ = tx.send(Message::Close(None)).await;
                    break;
                }
                Some(message) = control_rx.recv() => {
                    let Ok(message) = serde_json::to_string(&message) else {
                        continue;
                    };
                    if tx.send(Message::Text(message)).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Inbound task: WS → input/resize
    let inbound = tokio::spawn(async move {
        while let Some(msg) = rx.next().await {
            let Ok(msg) = msg else { break };
            match msg {
                Message::Text(text) => match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Input { data }) => {
                        if input.send(data.into_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Ok(ClientMsg::Resize { cols, rows }) => {
                        let _ = resize
                            .send(PtySize {
                                cols,
                                rows,
                                pixel_width: 0,
                                pixel_height: 0,
                            })
                            .await;
                    }
                    Ok(ClientMsg::Ping) => {
                        if control_tx.send(ServerMsg::Pong).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, "bad WS text message");
                    }
                },
                Message::Binary(bytes) => {
                    // Clients MAY send raw bytes as a shortcut for input.
                    if input.send(bytes).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
    });

    // Await either direction exiting; abort the other so we don't leak tasks.
    let mut outbound = outbound;
    let mut inbound = inbound;
    tokio::select! {
        _ = &mut outbound => inbound.abort(),
        _ = &mut inbound => outbound.abort(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ticket_is_bound_to_session_and_single_use() {
        let store = WsTicketStore::default();
        let session_id = Uuid::new_v4();
        let other_session_id = Uuid::new_v4();

        let wrong_session_ticket = store.issue(session_id).await.unwrap();
        assert!(!store.consume(&wrong_session_ticket, other_session_id).await);
        assert!(!store.consume(&wrong_session_ticket, session_id).await);

        let ticket = store.issue(session_id).await.unwrap();
        assert!(store.consume(&ticket, session_id).await);
        assert!(!store.consume(&ticket, session_id).await);

        let expired = store.issue_for(session_id, Duration::ZERO).await.unwrap();
        assert!(!store.consume(&expired, session_id).await);
    }

    #[test]
    fn ticket_is_read_from_websocket_protocol_not_url() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            "sulion.v1, sulion.ticket.opaque-ticket".parse().unwrap(),
        );
        assert_eq!(
            ticket_from_protocols(&headers).as_deref(),
            Some("opaque-ticket")
        );
    }
}
