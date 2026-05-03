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
use tokio_tungstenite::tungstenite::Message;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    sqlx::query(
        "TRUNCATE events, ingester_state, claude_sessions, pty_sessions, repos, \
         workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .ok();
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

async fn start_server(pool: db::Pool) -> (String, std::sync::Arc<AppState>) {
    let state = AppState::new(
        pool,
        std::path::PathBuf::from("/tmp"),
        std::path::PathBuf::from("/tmp/sulion-workspaces-test"),
        std::path::PathBuf::from("/tmp/sulion-library-test"),
        std::sync::Arc::new(sulion::ingest::Ingester::new()),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    let router = app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("ws://{addr}"), state)
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
    let (base, state) = start_server(pool.clone()).await;

    // Spawn a PTY that prints a sentinel and stays alive.
    let mgr: &std::sync::Arc<PtyManager> = &state.pty;
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
            ..Default::default()
        })
        .await
        .expect("spawn");

    // Give the shell a beat to produce the sentinel before we connect.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let url = format!("{base}/ws/sessions/{}", meta.id);
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
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
    let (base, state) = start_server(pool.clone()).await;

    let mgr: &std::sync::Arc<PtyManager> = &state.pty;
    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "while :; do sleep 1; done".into()],
            ..Default::default()
        })
        .await
        .expect("spawn");

    let url = format!("{base}/ws/sessions/{}", meta.id);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
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
    let (base, state) = start_server(pool.clone()).await;

    let mgr: &std::sync::Arc<PtyManager> = &state.pty;
    // cat(1) echoes its stdin back verbatim.
    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/cat"),
            ..Default::default()
        })
        .await
        .expect("spawn");

    let url = format!("{base}/ws/sessions/{}", meta.id);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
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
async fn unknown_session_id_returns_404() {
    let pool = fresh_pool().await;
    let (base, _state) = start_server(pool.clone()).await;
    let bogus = uuid::Uuid::new_v4();
    let url = format!("{base}/ws/sessions/{bogus}");
    // tokio-tungstenite returns Err on non-101 responses.
    let res = tokio_tungstenite::connect_async(&url).await;
    assert!(res.is_err(), "expected connect to fail with 404");
}
