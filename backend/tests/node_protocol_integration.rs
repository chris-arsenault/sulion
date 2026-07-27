#![cfg(feature = "integration-tests")]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};
use sulion::node_protocol::model::{ControlChallenge, ControlWireMessage, NodeWireMessage};
use sulion::node_protocol::{
    heartbeat_envelope, EnrollNodeRequest, NodeControl, NodeHello, NodeRequestKind, TerminalEvent,
    NODE_PROTOCOL_VERSION,
};
use sulion::node_runtime::{
    NodeRuntime, RepoCreateRequest, RepoPathRequest, ResourceRequest, SessionCreateRequest,
    SessionLaunch,
};
use sulion::{app, db, AppState};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

type ClientSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn fresh_pool() -> db::Pool {
    let url = std::env::var("SULION_TEST_DB").expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(&pool)
        .await
        .expect("drop schema");
    sqlx::query("CREATE SCHEMA public")
        .execute(&pool)
        .await
        .expect("create schema");
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

async fn start_server(pool: db::Pool, node_required: bool) -> (String, Arc<AppState>) {
    let state = AppState::new_with_auth_and_node_mode(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
        node_required,
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("listener address");
    let router = app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{addr}"), state)
}

fn generate_keypair() -> Ed25519KeyPair {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate key");
    Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("parse key")
}

async fn enroll(control: &NodeControl, keypair: &Ed25519KeyPair, node_id: Uuid) {
    let token = control
        .create_enrollment_token("test-node", node_id, Some(300))
        .await
        .expect("create enrollment token");
    let enrolled = control
        .enroll(EnrollNodeRequest {
            token: token.token,
            public_key: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(keypair.public_key().as_ref()),
        })
        .await
        .expect("enroll");
    assert_eq!(enrolled.node_id, node_id);
}

async fn open_socket(http_base: &str) -> ClientSocket {
    let ws_base = http_base.replacen("http://", "ws://", 1);
    tokio_tungstenite::connect_async(format!("{ws_base}/ws/nodes"))
        .await
        .expect("connect websocket")
        .0
}

async fn receive_challenge(socket: &mut ClientSocket) -> ControlChallenge {
    let frame = socket
        .next()
        .await
        .expect("challenge frame")
        .expect("challenge read");
    let Message::Text(text) = frame else {
        panic!("expected text challenge");
    };
    match serde_json::from_str(&text).expect("challenge json") {
        ControlWireMessage::Challenge { challenge } => challenge,
        _ => panic!("expected challenge"),
    }
}

fn signed_hello(
    keypair: &Ed25519KeyPair,
    challenge: &ControlChallenge,
    node_id: Uuid,
    boot_id: Uuid,
) -> NodeHello {
    let mut hello = NodeHello {
        node_id,
        boot_id,
        protocol_version: NODE_PROTOCOL_VERSION,
        signature: String::new(),
    };
    hello.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(keypair.sign(&hello.signing_payload(challenge)).as_ref());
    hello
}

async fn connect_node(
    http_base: &str,
    keypair: &Ed25519KeyPair,
    node_id: Uuid,
    boot_id: Uuid,
) -> ClientSocket {
    let mut socket = open_socket(http_base).await;
    let challenge = receive_challenge(&mut socket).await;
    let hello = signed_hello(keypair, &challenge, node_id, boot_id);
    let envelope = sulion::node_protocol::WireEnvelope::new(node_id, boot_id, "node.hello");
    socket
        .send(Message::Text(
            serde_json::to_string(&NodeWireMessage::Hello { envelope, hello }).expect("hello json"),
        ))
        .await
        .expect("send hello");
    let response = socket.next().await.expect("ack frame").expect("ack read");
    let Message::Text(text) = response else {
        panic!("expected text acknowledgment");
    };
    let ControlWireMessage::Envelope { envelope } = serde_json::from_str(&text).expect("ack json")
    else {
        panic!("expected acknowledgment envelope");
    };
    assert_eq!(envelope.message_kind, "control.hello_ack");
    assert_eq!(envelope.payload["protocol_version"], NODE_PROTOCOL_VERSION);
    socket
}

async fn send_heartbeat(
    socket: &mut ClientSocket,
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: Vec<Uuid>,
) {
    let envelope = heartbeat_envelope(node_id, boot_id, live_session_ids, true);
    socket
        .send(Message::Text(
            serde_json::to_string(&NodeWireMessage::Envelope { envelope }).expect("heartbeat json"),
        ))
        .await
        .expect("send heartbeat");
    tokio::time::sleep(Duration::from_millis(40)).await;
}

#[tokio::test]
async fn control_stays_ready_and_refuses_local_mutations_without_the_node() {
    let pool = fresh_pool().await;
    let (base, _state) = start_server(pool, true).await;
    let client = reqwest::Client::new();

    let health: Value = client
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("control health")
        .json()
        .await
        .expect("health json");
    assert_eq!(health["development_node"], "unavailable");

    let create = client
        .post(format!("{base}/api/repos"))
        .json(&json!({"name": "must-not-be-local"}))
        .send()
        .await
        .expect("control mutation");
    assert_eq!(create.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn fixed_node_enrollment_authenticates_one_connection() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool, false).await;
    let keypair = generate_keypair();
    let node_id = Uuid::new_v4();

    let token_response = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enrollment-tokens"))
        .json(&json!({
            "display_name": "sulion-enclave",
            "target_node_id": node_id,
            "ttl_seconds": 300,
        }))
        .send()
        .await
        .expect("token request");
    assert_eq!(token_response.status(), reqwest::StatusCode::CREATED);
    let token: Value = token_response.json().await.expect("token json");

    let enrolled = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enroll"))
        .json(&json!({
            "token": token["token"],
            "public_key": base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(keypair.public_key().as_ref()),
        }))
        .send()
        .await
        .expect("enroll request");
    assert_eq!(enrolled.status(), reqwest::StatusCode::CREATED);

    let _socket = connect_node(&base, &keypair, node_id, Uuid::new_v4()).await;
    assert_eq!(state.node_control.active_connection_count().await, 1);
    let node = state.node_control.list_nodes().await.unwrap().remove(0);
    assert_eq!(node.id, node_id);
    assert_eq!(node.connection_state, "connected");
}

#[tokio::test]
async fn same_boot_reconnect_preserves_sessions_and_new_boot_ends_them() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone(), false).await;
    let keypair = generate_keypair();
    let node_id = Uuid::new_v4();
    enroll(&state.node_control, &keypair, node_id).await;
    let first_boot = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, node_id, first_boot).await;
    let session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions \
            (id, repo, working_dir, state, node_id, node_boot_id) \
         VALUES ($1, 'repo', '/repo', 'live', $2, $3)",
    )
    .bind(session_id)
    .bind(node_id)
    .bind(first_boot)
    .execute(&pool)
    .await
    .unwrap();
    send_heartbeat(&mut socket, node_id, first_boot, vec![session_id]).await;

    socket.close(None).await.expect("close first connection");
    let _same_boot = connect_node(&base, &keypair, node_id, first_boot).await;
    let state_after_reconnect: String =
        sqlx::query_scalar("SELECT state FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state_after_reconnect, "live");

    let _new_boot = connect_node(&base, &keypair, node_id, Uuid::new_v4()).await;
    let row: (String, Option<String>) =
        sqlx::query_as("SELECT state, runtime_end_reason FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "dead");
    assert_eq!(row.1.as_deref(), Some("node_reboot"));
}

#[tokio::test]
async fn loopback_uses_the_same_direct_request_path_as_the_remote_node() {
    let pool = fresh_pool().await;
    let control = NodeControl::new(pool);
    let node_id = Uuid::new_v4();
    control
        .start_loopback(node_id, "standalone-test")
        .await
        .expect("start loopback");

    let response = control
        .request(node_id, NodeRequestKind::ProbeEcho, json!({"value": 42}))
        .await
        .expect("direct request");
    assert_eq!(response, json!({"echo": {"value": 42}}));
}

#[tokio::test]
async fn extracted_runtime_preserves_a_pty_across_control_replacement() {
    let pool = fresh_pool().await;
    let root = tempfile::tempdir().expect("runtime root");
    let repos_root = root.path().join("repos");
    let workspaces_root = root.path().join("workspaces");
    std::fs::create_dir_all(&repos_root).expect("repos root");
    std::fs::create_dir_all(&workspaces_root).expect("workspaces root");
    let node_id = Uuid::new_v4();
    let runtime = NodeRuntime::new(
        node_id,
        Uuid::new_v4(),
        pool.clone(),
        repos_root,
        workspaces_root,
    );
    let first_control = NodeControl::new(pool.clone());
    first_control
        .start_runtime_loopback(runtime.clone(), "runtime-test")
        .await
        .expect("connect runtime");

    first_control
        .request(
            node_id,
            NodeRequestKind::RepoCreate,
            serde_json::to_value(RepoCreateRequest {
                name: "restart-test".into(),
                git_url: None,
            })
            .unwrap(),
        )
        .await
        .expect("create repo");

    let session_id = Uuid::new_v4();
    first_control
        .request(
            node_id,
            NodeRequestKind::SessionCreate,
            serde_json::to_value(SessionCreateRequest {
                session_id,
                allocated_workspace_id: Uuid::new_v4(),
                existing_workspace_id: None,
                repo: "restart-test".into(),
                working_dir: None,
                workspace_mode: "main".into(),
                cols: 100,
                rows: 30,
                launch: SessionLaunch::Shell,
            })
            .unwrap(),
        )
        .await
        .expect("create session");

    let attachment = first_control
        .open_terminal(node_id, session_id)
        .await
        .expect("attach terminal");
    let (sender, mut events) = attachment.into_parts();
    wait_for_terminal_ready(&mut events).await;
    sender.close().await;

    let replacement_control = NodeControl::new(pool);
    replacement_control
        .start_runtime_loopback(runtime.clone(), "runtime-test")
        .await
        .expect("replace control");
    assert_eq!(runtime.pty().live_count().await, 1);

    replacement_control
        .request(
            node_id,
            NodeRequestKind::SessionDelete,
            serde_json::to_value(ResourceRequest { id: session_id }).unwrap(),
        )
        .await
        .expect("delete session");
}

#[tokio::test]
async fn node_file_requests_reject_traversal_and_symlink_escapes() {
    let pool = fresh_pool().await;
    let root = tempfile::tempdir().expect("runtime root");
    let repos_root = root.path().join("repos");
    let workspaces_root = root.path().join("workspaces");
    std::fs::create_dir_all(&repos_root).expect("repos root");
    std::fs::create_dir_all(&workspaces_root).expect("workspaces root");
    let node_id = Uuid::new_v4();
    let runtime = NodeRuntime::new(
        node_id,
        Uuid::new_v4(),
        pool.clone(),
        repos_root.clone(),
        workspaces_root,
    );
    let control = NodeControl::new(pool);
    control
        .start_runtime_loopback(runtime, "filesystem-test")
        .await
        .expect("connect runtime");
    control
        .request(
            node_id,
            NodeRequestKind::RepoCreate,
            serde_json::to_value(RepoCreateRequest {
                name: "filesystem-test".into(),
                git_url: None,
            })
            .unwrap(),
        )
        .await
        .expect("create repo");

    let outside = root.path().join("outside.txt");
    std::fs::write(&outside, "outside").expect("outside fixture");
    std::os::unix::fs::symlink(&outside, repos_root.join("filesystem-test/link.txt"))
        .expect("symlink fixture");

    for path in ["../outside.txt", "link.txt"] {
        let error = control
            .request(
                node_id,
                NodeRequestKind::RepoFileRaw,
                serde_json::to_value(RepoPathRequest {
                    repo: "filesystem-test".into(),
                    path: Some(path.into()),
                    all: false,
                })
                .unwrap(),
            )
            .await
            .expect_err("escaped path must fail");
        assert!(matches!(
            error,
            sulion::node_protocol::NodeProtocolError::Remote { .. }
        ));
    }
}

async fn wait_for_terminal_ready(events: &mut tokio::sync::mpsc::Receiver<TerminalEvent>) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("terminal ready timeout")
            .expect("terminal event stream closed")
        {
            TerminalEvent::Ready => return,
            TerminalEvent::Dead(code) => panic!("terminal died before ready: {code:?}"),
            TerminalEvent::Disconnected => panic!("terminal disconnected before ready"),
            TerminalEvent::Snapshot(_) | TerminalEvent::Output(_) => {}
        }
    }
}
