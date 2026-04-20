//! WebSocket attach route. Clients connect to `/ws/sessions/:id`, receive
//! the current shadow-emulator snapshot as the first binary frame, then
//! live PTY bytes as subsequent binary frames. Input and resize commands
//! are sent by the client as JSON text frames.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use futures::{sink::SinkExt, stream::StreamExt};
use portable_pty::PtySize;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::pty::PtySession;
use crate::AppState;

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
    /// PTY has exited. `exit` is the captured status if known.
    Dead { exit: Option<i32> },
}

pub async fn attach(
    Path(id): Path<Uuid>,
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let Some(session) = state.pty.get(id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "no such session").into_response();
    };
    let ws_test_hooks = state.ws_test_hooks.clone();
    ws.on_upgrade(move |socket| handle_socket(socket, session, ws_test_hooks))
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
                        // Handled by the outbound side? No — we only
                        // write snapshot/binary here. Ping/Pong via the
                        // control channel is rare; drop silently.
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
    tokio::select! {
        _ = outbound => {}
        _ = inbound => {}
    }
}
