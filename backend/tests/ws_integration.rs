#![cfg(feature = "integration-tests")]

//! WebSocket attach integration test. Spawns the full axum stack on a
//! random loopback port, connects a tungstenite client, and asserts the
//! snapshot + live stream + resize paths. Gated on `SULION_TEST_DB`.

use std::path::PathBuf;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use sulion::pty::{PtyManager, SpawnParams};
use sulion::{app, db, AppState};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Message;

mod common;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    sqlx::query(
        "TRUNCATE retrieval_embedding_backfills, retrieval_embedding_sources, retrieval_embeddings, \
         plan_events, plan_attachments, plan_phases, plans, session_activity_state, \
         events, ingester_state, claude_sessions, pty_sessions, repos, \
         workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .ok();
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

/// Returns the node runtime alongside the state: terminal attach is served by
/// the node that owns the PTY, so these tests must place their sessions on the
/// node's manager rather than the control plane's.
async fn start_server(
    pool: db::Pool,
) -> (
    String,
    std::sync::Arc<AppState>,
    std::sync::Arc<sulion::node_runtime::NodeRuntime>,
) {
    let (state, runtime) = common::state_with_loopback_node(
        pool,
        std::path::Path::new("/tmp"),
        std::path::Path::new("/tmp/sulion-workspaces-test"),
        std::path::Path::new("/tmp/sulion-library-test"),
    )
    .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let router = app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("ws://{addr}"), state, runtime)
}

async fn ticketed_request(base: &str, session_id: uuid::Uuid) -> Request<()> {
    let http_base = base.replacen("ws://", "http://", 1);
    let response = reqwest::Client::new()
        .post(format!("{http_base}/api/ws-tickets"))
        .json(&serde_json::json!({ "session_id": session_id }))
        .send()
        .await
        .expect("issue websocket ticket");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("ticket response");
    let ticket = body["ticket"].as_str().expect("ticket");
    let mut request = format!("{base}/ws/sessions/{session_id}")
        .into_client_request()
        .expect("websocket request");
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        format!("sulion.v1, sulion.ticket.{ticket}")
            .parse()
            .expect("protocol header"),
    );
    request
}

/// Read frames from the socket with a timeout; return whatever we got
/// before the timeout fired. Used to accumulate bytes without blocking
/// forever when the PTY has gone idle.
async fn collect_for(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    dur: Duration,
) -> Vec<Message> {
    let deadline = tokio::time::Instant::now() + dur;
    let mut out = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(msg))) => out.push(msg),
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => break,
        }
    }
    out
}

#[tokio::test]
async fn connect_receives_snapshot_then_ready_then_live_bytes() {
    let pool = fresh_pool().await;
    let (base, _state, runtime) = start_server(pool.clone()).await;

    // Spawn a PTY that prints a sentinel and stays alive.
    // The node's manager, not the control plane's: whoever spawns the process
    // is who the websocket layer asks to attach to it.
    let mgr: std::sync::Arc<PtyManager> = runtime.pty();
    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".into(),
                // Print sentinel, then loop sleeping so the PTY stays live.
                "printf 'SNAPSHOT_SENTINEL\\n'; while :; do sleep 1; done".into(),
            ],
            // Stamp the node identity the way the node's own create_session
            // does. Without it the row has no owner and the ticket route
            // cannot resolve which node to attach through.
            node_id: Some(runtime.node_id()),
            node_boot_id: Some(runtime.boot_id()),
            ..Default::default()
        })
        .await
        .expect("spawn");

    // Give the shell a beat to produce the sentinel before we connect.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let request = ticketed_request(&base, meta.id).await;
    let (mut socket, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws connect");

    let frames = collect_for(&mut socket, Duration::from_millis(1000)).await;
    assert!(!frames.is_empty(), "expected at least one frame (snapshot)");

    // First frame must be the binary snapshot.
    assert!(
        matches!(&frames[0], Message::Binary(_)),
        "first frame should be the binary snapshot, got {:?}",
        frames[0]
    );
    let snapshot_bytes = match &frames[0] {
        Message::Binary(b) => b.clone(),
        _ => unreachable!(),
    };
    let snap_str = String::from_utf8_lossy(&snapshot_bytes);
    assert!(
        snap_str.contains("SNAPSHOT_SENTINEL"),
        "snapshot should contain sentinel; got: {snap_str:?}"
    );

    // Somewhere among the frames there should be a text frame with `Ready`.
    let has_ready = frames.iter().any(|m| match m {
        Message::Text(t) => t.contains("ready"),
        _ => false,
    });
    assert!(has_ready, "expected a Ready text frame, got {frames:?}");

    // Clean up.
    let _ = socket.close(None).await;
    mgr.delete(meta.id).await.expect("delete");
}

#[tokio::test]
async fn resize_message_is_accepted() {
    let pool = fresh_pool().await;
    let (base, _state, runtime) = start_server(pool.clone()).await;

    // The node's manager, not the control plane's: whoever spawns the process
    // is who the websocket layer asks to attach to it.
    let mgr: std::sync::Arc<PtyManager> = runtime.pty();
    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "while :; do sleep 1; done".into()],
            // Stamp the node identity the way the node's own create_session
            // does. Without it the row has no owner and the ticket route
            // cannot resolve which node to attach through.
            node_id: Some(runtime.node_id()),
            node_boot_id: Some(runtime.boot_id()),
            ..Default::default()
        })
        .await
        .expect("spawn");

    let request = ticketed_request(&base, meta.id).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");

    // Drain snapshot + ready.
    let _ = collect_for(&mut socket, Duration::from_millis(200)).await;

    // Send a resize message. The session's emulator should accept it
    // without panicking; the server has no way to reject a resize so we
    // just verify the socket stays open afterwards.
    socket
        .send(Message::Text(
            r#"{"t":"resize","cols":160,"rows":48}"#.into(),
        ))
        .await
        .expect("send resize");

    // Send a bogus message — server should log and ignore, not crash.
    socket
        .send(Message::Text(r#"{"t":"garbage"}"#.into()))
        .await
        .expect("send garbage");

    socket
        .send(Message::Text(r#"{"t":"ping"}"#.into()))
        .await
        .expect("send application ping");

    let heartbeat_frames = collect_for(&mut socket, Duration::from_millis(500)).await;
    assert!(
        heartbeat_frames
            .iter()
            .any(|frame| matches!(frame, Message::Text(text) if text.contains("pong"))),
        "expected application pong, got {heartbeat_frames:?}"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Socket should still be healthy: ping round-trip works.
    socket
        .send(Message::Ping(b"hi".to_vec()))
        .await
        .expect("ping");

    let _ = socket.close(None).await;
    mgr.delete(meta.id).await.expect("delete");
}

#[tokio::test]
async fn input_sent_to_shell_appears_in_output() {
    let pool = fresh_pool().await;
    let (base, _state, runtime) = start_server(pool.clone()).await;

    // The node's manager, not the control plane's: whoever spawns the process
    // is who the websocket layer asks to attach to it.
    let mgr: std::sync::Arc<PtyManager> = runtime.pty();
    // cat(1) echoes its stdin back verbatim.
    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/cat"),
            // Stamp the node identity the way the node's own create_session
            // does. Without it the row has no owner and the ticket route
            // cannot resolve which node to attach through.
            node_id: Some(runtime.node_id()),
            node_boot_id: Some(runtime.boot_id()),
            ..Default::default()
        })
        .await
        .expect("spawn");

    let request = ticketed_request(&base, meta.id).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect");

    // Drain snapshot + ready.
    let _ = collect_for(&mut socket, Duration::from_millis(300)).await;

    // Send input — cat will echo it back.
    socket
        .send(Message::Text(
            r#"{"t":"input","data":"echo-sentinel\n"}"#.into(),
        ))
        .await
        .expect("send input");

    let frames = collect_for(&mut socket, Duration::from_millis(1000)).await;
    let mut saw_echo = false;
    for f in frames {
        if let Message::Binary(bytes) = f {
            if String::from_utf8_lossy(&bytes).contains("echo-sentinel") {
                saw_echo = true;
                break;
            }
        }
    }
    assert!(saw_echo, "expected cat to echo input back through the WS");

    let _ = socket.close(None).await;
    mgr.delete(meta.id).await.expect("delete");
}

#[tokio::test]
async fn websocket_requires_a_ticket() {
    let pool = fresh_pool().await;
    let (base, _state, _runtime) = start_server(pool.clone()).await;
    let bogus = uuid::Uuid::new_v4();
    let url = format!("{base}/ws/sessions/{bogus}");
    // tokio-tungstenite returns Err on non-101 responses.
    let res = tokio_tungstenite::connect_async(&url).await;
    assert!(res.is_err(), "expected connect without a ticket to fail");
}

#[tokio::test]
async fn ticket_is_single_use() {
    let pool = fresh_pool().await;
    let (base, _state, runtime) = start_server(pool.clone()).await;
    let mgr = runtime.pty();
    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "while :; do sleep 1; done".into()],
            // Stamp the node identity the way the node's own create_session
            // does. Without it the row has no owner and the ticket route
            // cannot resolve which node to attach through.
            node_id: Some(runtime.node_id()),
            node_boot_id: Some(runtime.boot_id()),
            ..Default::default()
        })
        .await
        .expect("spawn");

    let request = ticketed_request(&base, meta.id).await;
    let mut replay = request
        .uri()
        .to_string()
        .into_client_request()
        .expect("replay request");
    replay.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        request.headers()[SEC_WEBSOCKET_PROTOCOL].clone(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("first use succeeds");
    let _ = socket.close(None).await;

    let second = tokio_tungstenite::connect_async(replay).await;
    assert!(second.is_err(), "reusing a websocket ticket must fail");
    mgr.delete(meta.id).await.expect("delete");
}
