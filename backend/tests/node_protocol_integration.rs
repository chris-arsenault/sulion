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
    heartbeat_envelope, NodeControl, NodeHello, NodeLanGuard, NodeRequestKind, NodeRuntimeConfig,
    NodeSourcePolicy, TerminalEvent, DEDICATED_NODE_ID, NODE_PROTOCOL_VERSION,
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

/// Serves with peer addresses attached, which is what the node source boundary
/// reads. The plain helper above leaves them off, matching the tests that do
/// not exercise admission.
async fn start_server_with_peer_addresses(
    pool: db::Pool,
    delivered_config: Option<NodeRuntimeConfig>,
    source_policy: NodeSourcePolicy,
) -> (String, Arc<AppState>) {
    start_server_with_identity(pool, delivered_config, source_policy, None).await
}

async fn start_server_with_identity(
    pool: db::Pool,
    delivered_config: Option<NodeRuntimeConfig>,
    source_policy: NodeSourcePolicy,
    control_identity: Option<sulion::node_protocol::ControlIdentity>,
) -> (String, Arc<AppState>) {
    let state = AppState::new_with_auth_and_node_mode(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
        true,
    );
    state
        .node_control
        .apply_policy(delivered_config, source_policy, control_identity, None)
        .expect("apply node policy");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("listener address");
    let router = app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    (format!("http://{addr}"), state)
}

fn delivered_config(pairs: &[(&str, &str)]) -> NodeRuntimeConfig {
    NodeRuntimeConfig::new(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

/// Reads the configuration envelope the control plane sends after the
/// acknowledgment.
async fn receive_node_config(socket: &mut ClientSocket) -> Value {
    let frame = socket
        .next()
        .await
        .expect("config frame")
        .expect("config read");
    let Message::Text(text) = frame else {
        panic!("expected text configuration");
    };
    let ControlWireMessage::Envelope { envelope } =
        serde_json::from_str(&text).expect("config json")
    else {
        panic!("expected configuration envelope");
    };
    assert_eq!(envelope.message_kind, "control.node_config");
    envelope.payload
}

fn generate_keypair() -> Ed25519KeyPair {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate key");
    Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("parse key")
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
    signed_hello_with_nonce(keypair, challenge, node_id, boot_id, "node-nonce")
}

fn signed_hello_with_nonce(
    keypair: &Ed25519KeyPair,
    challenge: &ControlChallenge,
    node_id: Uuid,
    boot_id: Uuid,
    node_nonce: &str,
) -> NodeHello {
    let mut hello = NodeHello {
        node_id,
        boot_id,
        node_nonce: node_nonce.to_string(),
        tunnel_public_key: None,
        protocol_version: NODE_PROTOCOL_VERSION,
        public_key: Some(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(keypair.public_key().as_ref()),
        ),
        signature: String::new(),
    };
    hello.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(keypair.sign(&hello.signing_payload(challenge)).as_ref());
    hello
}

async fn request_pairing(http_base: &str, keypair: &Ed25519KeyPair, node_id: Uuid) {
    let boot_id = Uuid::new_v4();
    let mut socket = open_socket(http_base).await;
    let challenge = receive_challenge(&mut socket).await;
    let hello = signed_hello(keypair, &challenge, node_id, boot_id);
    let envelope = sulion::node_protocol::WireEnvelope::new(node_id, boot_id, "node.hello");
    socket
        .send(Message::Text(
            serde_json::to_string(&NodeWireMessage::Hello { envelope, hello }).expect("hello json"),
        ))
        .await
        .expect("send pairing hello");
    let response = socket
        .next()
        .await
        .expect("pairing response")
        .expect("pairing response read");
    let Message::Close(Some(frame)) = response else {
        panic!("expected pairing-required close frame");
    };
    assert_eq!(frame.reason, "node approval required");
}

async fn pair_and_approve(http_base: &str, control: &NodeControl, keypair: &Ed25519KeyPair) {
    request_pairing(http_base, keypair, DEDICATED_NODE_ID).await;
    control
        .approve_pairing(DEDICATED_NODE_ID)
        .await
        .expect("approve pairing");
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
async fn fixed_node_pairing_is_approved_in_the_control_plane() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool, false).await;
    let keypair = generate_keypair();
    request_pairing(&base, &keypair, DEDICATED_NODE_ID).await;
    let pending = state.node_control.list_nodes().await.unwrap().remove(0);
    assert_eq!(pending.id, DEDICATED_NODE_ID);
    assert_eq!(pending.connection_state, "pending");
    assert!(pending
        .pending_key_fingerprint
        .as_deref()
        .is_some_and(|value| value.starts_with("SHA256:")));

    let approved = reqwest::Client::new()
        .post(format!("{base}/api/nodes/{DEDICATED_NODE_ID}/approve"))
        .send()
        .await
        .expect("approve pairing request");
    assert_eq!(approved.status(), reqwest::StatusCode::NO_CONTENT);

    let _socket = connect_node(&base, &keypair, DEDICATED_NODE_ID, Uuid::new_v4()).await;
    assert_eq!(state.node_control.active_connection_count().await, 1);
    let node = state.node_control.list_nodes().await.unwrap().remove(0);
    assert_eq!(node.id, DEDICATED_NODE_ID);
    assert_eq!(node.connection_state, "connected");
    assert_eq!(node.pending_key_fingerprint, None);
}

#[tokio::test]
async fn same_boot_reconnect_preserves_sessions_and_new_boot_ends_them() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone(), false).await;
    let keypair = generate_keypair();
    let node_id = DEDICATED_NODE_ID;
    pair_and_approve(&base, &state.node_control, &keypair).await;
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

#[tokio::test]
async fn an_approved_node_receives_the_runtime_configuration_it_was_never_given() {
    // The whole point of the bootstrap: a machine that holds only its identity
    // key ends up with everything it needs to run, without anyone copying a
    // credential onto it.
    let pool = fresh_pool().await;
    let config = delivered_config(&[
        ("DB_PASSWORD", "delivered-secret"),
        ("DB_USER", "sulion"),
        ("SULION_RETRIEVAL_TOKEN", "retrieval-token"),
    ]);
    let (base, state) = start_server_with_peer_addresses(
        pool,
        Some(config.clone()),
        NodeSourcePolicy::new(
            NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
    )
    .await;
    let keypair = generate_keypair();

    pair_and_approve(&base, &state.node_control, &keypair).await;
    let mut socket = connect_node(&base, &keypair, DEDICATED_NODE_ID, Uuid::new_v4()).await;

    let payload = receive_node_config(&mut socket).await;
    assert_eq!(payload["digest"], config.digest());
    assert_eq!(payload["values"]["DB_USER"], "sulion");
    assert_eq!(payload["values"]["DB_PASSWORD"], "delivered-secret");
    assert_eq!(
        payload["values"]["SULION_RETRIEVAL_TOKEN"],
        "retrieval-token"
    );
}

#[tokio::test]
async fn an_unapproved_node_is_told_nothing() {
    // Pairing must not be a way to read credentials: the connection is closed
    // before any configuration is written to it.
    let pool = fresh_pool().await;
    let (base, _state) = start_server_with_peer_addresses(
        pool,
        Some(delivered_config(&[("DB_PASSWORD", "delivered-secret")])),
        NodeSourcePolicy::new(
            NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
    )
    .await;
    let keypair = generate_keypair();

    // Closes with "node approval required" and nothing else.
    request_pairing(&base, &keypair, DEDICATED_NODE_ID).await;
}

#[tokio::test]
async fn a_node_outside_the_lan_never_reaches_the_challenge() {
    let pool = fresh_pool().await;
    // The loopback source this test connects from is outside the configured
    // boundary, which is the shape of a node dialling in from off-LAN.
    let (base, _state) = start_server_with_peer_addresses(
        pool,
        Some(delivered_config(&[("DB_PASSWORD", "delivered-secret")])),
        NodeSourcePolicy::new(
            NodeLanGuard::parse("192.168.66.0/24").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
    )
    .await;

    let ws_base = base.replacen("http://", "ws://", 1);
    let error = tokio_tungstenite::connect_async(format!("{ws_base}/ws/nodes"))
        .await
        .expect_err("off-LAN node must be refused");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected an HTTP refusal, got {error:?}");
    };
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn a_forged_client_address_does_not_get_a_node_onto_the_lan() {
    let pool = fresh_pool().await;
    // No trusted proxies are configured, so the header is ignored entirely and
    // the real loopback peer is what the boundary sees.
    let (base, _state) = start_server_with_peer_addresses(
        pool,
        None,
        NodeSourcePolicy::new(
            NodeLanGuard::parse("192.168.66.0/24").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
    )
    .await;

    let ws_base = base.replacen("http://", "ws://", 1);
    let request = http::Request::builder()
        .uri(format!("{ws_base}/ws/nodes"))
        .header("Host", "sulion.test")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("X-Real-IP", "192.168.66.4")
        .body(())
        .expect("build upgrade request");
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("forged client address must be refused");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected an HTTP refusal, got {error:?}");
    };
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn a_deployment_with_nothing_to_deliver_says_so_immediately() {
    // The portable and test deployments configure nodes some other way. They
    // must still answer, so a node learns there is nothing coming instead of
    // waiting out a timeout on every start.
    let pool = fresh_pool().await;
    let (base, state) = start_server_with_peer_addresses(
        pool,
        None,
        NodeSourcePolicy::new(
            NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
    )
    .await;
    let keypair = generate_keypair();

    pair_and_approve(&base, &state.node_control, &keypair).await;
    let mut socket = connect_node(&base, &keypair, DEDICATED_NODE_ID, Uuid::new_v4()).await;

    let payload = receive_node_config(&mut socket).await;
    assert_eq!(payload["values"], json!({}));
}

/// Reads the acknowledgment without asserting on it, for tests that care about
/// what control proved rather than that it acknowledged.
async fn receive_ack(socket: &mut ClientSocket) -> sulion::node_protocol::model::HelloAck {
    let frame = socket.next().await.expect("ack frame").expect("ack read");
    let Message::Text(text) = frame else {
        panic!("expected text acknowledgment");
    };
    let ControlWireMessage::Envelope { envelope } = serde_json::from_str(&text).expect("ack json")
    else {
        panic!("expected acknowledgment envelope");
    };
    assert_eq!(envelope.message_kind, "control.hello_ack");
    serde_json::from_value(envelope.payload).expect("ack payload")
}

fn control_identity() -> sulion::node_protocol::ControlIdentity {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate");
    sulion::node_protocol::ControlIdentity::from_pkcs8(document.as_ref()).expect("identity")
}

#[tokio::test]
async fn control_proves_its_identity_and_signs_what_it_delivers() {
    // The node handshake proves who the node is; this is the other direction,
    // and it is what lets an already-paired node refuse a different peer.
    let pool = fresh_pool().await;
    let identity = control_identity();
    let expected_key = identity.public_key().to_string();
    let config = delivered_config(&[("DB_PASSWORD", "delivered-secret")]);
    let (base, state) = start_server_with_identity(
        pool,
        Some(config.clone()),
        NodeSourcePolicy::new(
            NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
        Some(identity),
    )
    .await;
    let keypair = generate_keypair();
    pair_and_approve(&base, &state.node_control, &keypair).await;

    let boot_id = Uuid::new_v4();
    let mut socket = open_socket(&base).await;
    let challenge = receive_challenge(&mut socket).await;
    let hello = signed_hello_with_nonce(
        &keypair,
        &challenge,
        DEDICATED_NODE_ID,
        boot_id,
        "connection-nonce",
    );
    let envelope =
        sulion::node_protocol::WireEnvelope::new(DEDICATED_NODE_ID, boot_id, "node.hello");
    socket
        .send(Message::Text(
            serde_json::to_string(&NodeWireMessage::Hello {
                envelope,
                hello: hello.clone(),
            })
            .expect("hello json"),
        ))
        .await
        .expect("send hello");

    let ack = receive_ack(&mut socket).await;
    let proof = ack.control_proof.expect("control must prove its identity");
    assert_eq!(proof.public_key, expected_key);

    // A node pins exactly this, so it must verify against the connection it
    // was made for.
    let pin_directory = tempfile::tempdir().expect("pin dir");
    let pin = sulion::node_protocol::ControlPin::new(pin_directory.path().join("control-key.pub"));
    assert_eq!(
        pin.verify(Some(&proof), &challenge, &hello)
            .expect("verify"),
        sulion::node_protocol::PinOutcome::FirstPairing(expected_key.clone())
    );
    pin.record(&expected_key).expect("record pin");

    // The delivered payload carries its own signature, bound to this nonce.
    let payload = receive_node_config(&mut socket).await;
    pin.verify_config(
        payload["digest"].as_str().expect("digest"),
        "connection-nonce",
        payload["signature"].as_str(),
    )
    .expect("configuration signature must verify against the pinned identity");
}

#[tokio::test]
async fn a_node_paired_to_one_control_plane_refuses_another() {
    let pool = fresh_pool().await;
    let (base, state) = start_server_with_identity(
        pool,
        None,
        NodeSourcePolicy::new(
            NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
        Some(control_identity()),
    )
    .await;
    let keypair = generate_keypair();
    pair_and_approve(&base, &state.node_control, &keypair).await;

    let boot_id = Uuid::new_v4();
    let mut socket = open_socket(&base).await;
    let challenge = receive_challenge(&mut socket).await;
    let hello = signed_hello(&keypair, &challenge, DEDICATED_NODE_ID, boot_id);
    let envelope =
        sulion::node_protocol::WireEnvelope::new(DEDICATED_NODE_ID, boot_id, "node.hello");
    socket
        .send(Message::Text(
            serde_json::to_string(&NodeWireMessage::Hello {
                envelope,
                hello: hello.clone(),
            })
            .expect("hello json"),
        ))
        .await
        .expect("send hello");
    let ack = receive_ack(&mut socket).await;
    let proof = ack.control_proof.expect("proof");

    // This node was paired to a different control plane, so the proof this one
    // makes is well-formed and still refused.
    let pin_directory = tempfile::tempdir().expect("pin dir");
    let pin = sulion::node_protocol::ControlPin::new(pin_directory.path().join("control-key.pub"));
    pin.record(control_identity().public_key())
        .expect("pin another");
    let error = pin
        .verify(Some(&proof), &challenge, &hello)
        .expect_err("a different control plane must be refused");
    assert!(error.to_string().contains("identity changed"));
}

#[tokio::test]
async fn credentials_are_withheld_from_the_cleartext_enrollment_hop() {
    // The tunnel cannot exist before the keys are exchanged, so enrollment has
    // to happen in the clear. That hop is allowed to carry public keys and the
    // peering, and nothing else.
    std::env::set_var("SULION_TUNNEL_ENDPOINT", "192.168.66.3:51820");
    std::env::set_var("SULION_TUNNEL_SUBNET", "10.88.0.0/24");
    let pool = fresh_pool().await;
    sqlx::query("INSERT INTO control_tunnel (id, private_key, public_key) VALUES (1, $1, $2)")
        .bind(vec![7_u8; 32])
        .bind(vec![9_u8; 32])
        .execute(&pool)
        .await
        .expect("seed control tunnel key");

    let tunnel = sulion::node_protocol::TunnelPolicy::load(&pool)
        .await
        .expect("load tunnel")
        .expect("tunnel configured");
    let identity = control_identity();
    let state = AppState::new_with_auth_and_node_mode(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
        true,
    );
    state
        .node_control
        .apply_policy(
            Some(delivered_config(&[("DB_PASSWORD", "delivered-secret")])),
            NodeSourcePolicy::new(
                NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
                NodeLanGuard::parse("").expect("proxies"),
            ),
            Some(identity),
            Some(tunnel),
        )
        .expect("apply policy");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let router = app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    let base = format!("http://{addr}");
    let keypair = generate_keypair();
    pair_and_approve(&base, &state.node_control, &keypair).await;

    let mut socket = connect_node(&base, &keypair, DEDICATED_NODE_ID, Uuid::new_v4()).await;
    // Loopback is not a tunnel address, so this connection gets the peering it
    // needs to build the tunnel and no credentials.
    let payload = receive_node_config(&mut socket).await;
    assert_eq!(
        payload["values"],
        json!({}),
        "credentials must not cross the cleartext enrollment hop"
    );

    std::env::remove_var("SULION_TUNNEL_ENDPOINT");
    std::env::remove_var("SULION_TUNNEL_SUBNET");
}

#[tokio::test]
async fn a_node_from_the_previous_release_still_authenticates() {
    // Deploys are control-plane first: CI only advances node-release after the
    // control deploy succeeds, so a still-running node from the previous
    // release has to keep connecting to the new control plane. If it could
    // not, the enclave would drop on every release until its poller caught up.
    let pool = fresh_pool().await;
    let (base, state) = start_server_with_identity(
        pool,
        None,
        NodeSourcePolicy::new(
            NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
            NodeLanGuard::parse("").expect("proxies"),
        ),
        Some(control_identity()),
    )
    .await;
    let keypair = generate_keypair();

    // A hello exactly as the previous release built it: no nonce, no tunnel
    // key, and a signature over only the fields that release covered.
    let legacy_hello = |challenge: &ControlChallenge, boot_id: Uuid| {
        let mut hello = NodeHello {
            node_id: DEDICATED_NODE_ID,
            boot_id,
            protocol_version: NODE_PROTOCOL_VERSION,
            public_key: Some(
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(keypair.public_key().as_ref()),
            ),
            node_nonce: String::new(),
            tunnel_public_key: None,
            signature: String::new(),
        };
        hello.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(keypair.sign(&hello.signing_payload(challenge)).as_ref());
        hello
    };

    let send_legacy = |base: String, boot_id: Uuid| async move {
        let mut socket = open_socket(&base).await;
        let challenge = receive_challenge(&mut socket).await;
        let hello = legacy_hello(&challenge, boot_id);
        let envelope =
            sulion::node_protocol::WireEnvelope::new(DEDICATED_NODE_ID, boot_id, "node.hello");
        socket
            .send(Message::Text(
                serde_json::to_string(&NodeWireMessage::Hello { envelope, hello })
                    .expect("hello json"),
            ))
            .await
            .expect("send legacy hello");
        socket
    };

    // Pairing, then a real connection, both with the old signature shape.
    let mut socket = send_legacy(base.clone(), Uuid::new_v4()).await;
    let response = socket.next().await.expect("frame").expect("read");
    let Message::Close(Some(frame)) = response else {
        panic!("expected pairing-required close, got {response:?}");
    };
    assert_eq!(frame.reason, "node approval required");
    state
        .node_control
        .approve_pairing(DEDICATED_NODE_ID)
        .await
        .expect("approve");

    let mut socket = send_legacy(base, Uuid::new_v4()).await;
    let ack = receive_ack(&mut socket).await;
    assert_eq!(ack.protocol_version, NODE_PROTOCOL_VERSION);
}
