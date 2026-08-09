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
    heartbeat_envelope, NodeControl, NodeHello, NodeHostStats, NodeLanGuard, NodeRequestKind,
    NodeRuntimeConfig, NodeSourcePolicy, TerminalEvent, DEDICATED_NODE_ID, NODE_PROTOCOL_VERSION,
};
use sulion::node_runtime::{
    NodeRuntime, RepoCreateRequest, RepoPathRequest, ResourceRequest, SessionCreateRequest,
    SessionLaunch,
};

mod common;
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

async fn start_server(pool: db::Pool) -> (String, Arc<AppState>) {
    let state = AppState::new_with_auth(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
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
    let state = AppState::new_with_auth(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
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
    send_heartbeat_with_host(socket, node_id, boot_id, live_session_ids, None).await;
}

async fn send_heartbeat_with_host(
    socket: &mut ClientSocket,
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: Vec<Uuid>,
    host: Option<NodeHostStats>,
) {
    let envelope = heartbeat_envelope(node_id, boot_id, live_session_ids, true, host, None);
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
    let (base, _state) = start_server(pool).await;
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
    let (base, state) = start_server(pool).await;
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
async fn an_old_connected_node_refuses_collection_launch_before_dispatch() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone()).await;
    for repo in ["alpha", "beta"] {
        sqlx::query(
            "INSERT INTO repo_runtime_state (repo_name, path, exists, next_status_at) \
             VALUES ($1, $2, TRUE, NOW())",
        )
        .bind(repo)
        .bind(format!("/tmp/{repo}"))
        .execute(&pool)
        .await
        .unwrap();
    }
    let group_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO meta_repos (id, name, primary_repo_name) VALUES ($1, 'Platform', 'alpha')",
    )
    .bind(group_id)
    .execute(&pool)
    .await
    .unwrap();
    for (position, repo) in ["alpha", "beta"].into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO meta_repo_members (meta_repo_id, repo_name, position) \
             VALUES ($1, $2, $3)",
        )
        .bind(group_id)
        .bind(repo)
        .bind(position as i32)
        .execute(&pool)
        .await
        .unwrap();
    }

    let keypair = generate_keypair();
    pair_and_approve(&base, &state.node_control, &keypair).await;
    let boot_id = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, DEDICATED_NODE_ID, boot_id).await;
    send_heartbeat(&mut socket, DEDICATED_NODE_ID, boot_id, Vec::new()).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/sessions"))
        .json(&json!({"meta_repo_id": group_id, "workspace_mode": "main"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("must finish updating"),
        "{body}",
    );
}

/// The strip's memory/CPU must describe the machine that runs PTYs and
/// builds. Control samples nothing of its own, so a deployment with no node
/// reports no machine rather than quietly measuring the control plane.
#[tokio::test]
async fn app_state_reports_the_nodes_machine_and_forgets_it_on_disconnect() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool).await;
    let keypair = generate_keypair();
    let node_id = DEDICATED_NODE_ID;
    pair_and_approve(&base, &state.node_control, &keypair).await;
    let client = reqwest::Client::new();

    // app-state serves the cached sample; the background sampler owns the
    // cadence in production, so the test drives it explicitly.
    sulion::api::sample_stats_once(&state)
        .await
        .expect("sample before node");
    let before: Value = client
        .get(format!("{base}/api/app-state"))
        .send()
        .await
        .expect("app-state before node")
        .json()
        .await
        .expect("app-state json");
    assert_eq!(before["stats"]["node"], Value::Null);

    let boot_id = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, node_id, boot_id).await;
    send_heartbeat_with_host(
        &mut socket,
        node_id,
        boot_id,
        Vec::new(),
        Some(NodeHostStats {
            memory_used_bytes: 11 * 1024 * 1024 * 1024,
            memory_total_bytes: 32 * 1024 * 1024 * 1024,
            cpu_percent: 47.5,
        }),
    )
    .await;

    sulion::api::sample_stats_once(&state)
        .await
        .expect("sample with node");
    let connected: Value = client
        .get(format!("{base}/api/app-state"))
        .send()
        .await
        .expect("app-state with node")
        .json()
        .await
        .expect("app-state json");
    let reported = &connected["stats"]["node"];
    assert_eq!(reported["memory_used_bytes"], 11 * 1024 * 1024 * 1024_u64);
    assert_eq!(reported["memory_total_bytes"], 32 * 1024 * 1024 * 1024_u64);
    assert_eq!(reported["cpu_percent"], 47.5);

    socket.close(None).await.expect("close node connection");
    tokio::time::sleep(Duration::from_millis(80)).await;
    sulion::api::sample_stats_once(&state)
        .await
        .expect("sample after disconnect");
    let after: Value = client
        .get(format!("{base}/api/app-state"))
        .send()
        .await
        .expect("app-state after disconnect")
        .json()
        .await
        .expect("app-state json");
    assert_eq!(after["stats"]["node"], Value::Null);
}

#[tokio::test]
async fn same_boot_reconnect_preserves_sessions_and_a_new_boot_defers_to_inventory() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone()).await;
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

    // A new boot's hello no longer ends prior-boot sessions — shells can
    // outlive a node restart in the devenv. The first complete inventory is
    // what decides; here it reports nothing hosted, so the session is
    // orphaned (resumable), not dead — no exit was ever reported.
    let new_boot = Uuid::new_v4();
    let mut new_socket = connect_node(&base, &keypair, node_id, new_boot).await;
    let state_after_new_boot: String =
        sqlx::query_scalar("SELECT state FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state_after_new_boot, "live");

    send_heartbeat(&mut new_socket, node_id, new_boot, vec![]).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let row: (String, Option<String>) =
            sqlx::query_as("SELECT state, runtime_end_reason FROM pty_sessions WHERE id = $1")
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        if row.0 == "orphaned" && row.1.as_deref() == Some("node_inventory_missing") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "empty inventory did not end the session: {row:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
    let (link, events) = common::in_process_devenv().await;
    let runtime = NodeRuntime::new(
        node_id,
        Uuid::new_v4(),
        pool.clone(),
        repos_root,
        workspaces_root,
        link,
        events,
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
                meta_repo: None,
                additional_repos: Vec::new(),
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
async fn a_node_restart_adopts_sessions_the_devenv_kept_alive() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone()).await;
    let keypair = generate_keypair();
    pair_and_approve(&base, &state.node_control, &keypair).await;

    // Two sessions created under the previous boot. The devenv kept one shell
    // alive across the node restart; the other died with it.
    let survivor = Uuid::new_v4();
    let casualty = Uuid::new_v4();
    let boot_a = Uuid::new_v4();
    for id in [survivor, casualty] {
        sqlx::query(
            "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at, node_id, node_boot_id) \
             VALUES ($1, 'r', '/tmp', 'live', NOW(), $2, $3)",
        )
        .bind(id)
        .bind(DEDICATED_NODE_ID)
        .bind(boot_a)
        .execute(&pool)
        .await
        .expect("insert prior-boot session");
    }

    // The node reconnects under a new boot id. The hello alone must no
    // longer end prior-boot sessions — that decision now belongs to the
    // first complete inventory.
    let boot_b = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, DEDICATED_NODE_ID, boot_b).await;
    let (state_after_hello,): (String,) =
        sqlx::query_as("SELECT state FROM pty_sessions WHERE id = $1")
            .bind(survivor)
            .fetch_one(&pool)
            .await
            .expect("read survivor after hello");
    assert_eq!(
        state_after_hello, "live",
        "hello must not end sessions the devenv may still host"
    );

    // First complete heartbeat: the devenv still hosts only the survivor.
    send_heartbeat(&mut socket, DEDICATED_NODE_ID, boot_b, vec![survivor]).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (survivor_state, survivor_boot): (String, Option<Uuid>) =
            sqlx::query_as("SELECT state, node_boot_id FROM pty_sessions WHERE id = $1")
                .bind(survivor)
                .fetch_one(&pool)
                .await
                .expect("read survivor");
        let (casualty_state, casualty_reason): (String, Option<String>) =
            sqlx::query_as("SELECT state, runtime_end_reason FROM pty_sessions WHERE id = $1")
                .bind(casualty)
                .fetch_one(&pool)
                .await
                .expect("read casualty");
        if survivor_state == "live"
            && survivor_boot == Some(boot_b)
            && casualty_state == "orphaned"
            && casualty_reason.as_deref() == Some("node_inventory_missing")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "inventory heartbeat did not reconcile: survivor {survivor_state}/{survivor_boot:?}, \
             casualty {casualty_state}/{casualty_reason:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
    let (link, events) = common::in_process_devenv().await;
    let runtime = NodeRuntime::new(
        node_id,
        Uuid::new_v4(),
        pool.clone(),
        repos_root.clone(),
        workspaces_root,
        link,
        events,
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
async fn the_node_channel_runs_over_tls_bound_to_the_control_identity() {
    // Sessions carry credentials, so the node channel must be encrypted and
    // the encryption must be attributable: the certificate a node sees is
    // signed into the handshake proof by the identity an operator approved.
    use sulion::node_protocol::tls;

    let pool = fresh_pool().await;
    let identity = control_identity();
    let expected_key = identity.public_key().to_string();
    let (cert_pem, key_pem) = tls::test_certificate();
    let control_tls = tls::ControlTls::from_pem(&cert_pem, &key_pem).expect("parse tls");
    let expected_digest = control_tls.digest.clone();

    let state = AppState::new_with_auth(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
    );
    state
        .node_control
        .apply_policy(
            None,
            NodeSourcePolicy::new(
                NodeLanGuard::parse("127.0.0.0/8").expect("lan"),
                NodeLanGuard::parse("").expect("proxies"),
            ),
            Some(identity),
            Some(expected_digest.clone()),
        )
        .expect("apply policy");

    let router = axum::Router::new()
        .merge(sulion::node_protocol::public_router())
        .with_state(state.clone());
    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(
        control_tls.server_config().expect("server config"),
    ));
    let handle = axum_server::Handle::new();
    let serve_handle = handle.clone();
    tokio::spawn(async move {
        axum_server::bind_rustls("127.0.0.1:0".parse().unwrap(), rustls_config)
            .handle(serve_handle)
            .serve(router.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .await
            .expect("serve tls");
    });
    let addr = handle.listening().await.expect("listening");
    let url = format!("wss://127.0.0.1:{}/ws/nodes", addr.port());

    let keypair = generate_keypair();

    // First contact: no pin yet, so the verifier records what it saw and the
    // signed proof is what judges it.
    let connect = |pinned: Option<rustls::pki_types::CertificateDer<'static>>| {
        let url = url.clone();
        async move {
            let (verifier, seen) = tls::PinnedServerVerifier::new(pinned);
            let connector = tokio_tungstenite::Connector::Rustls(Arc::new(
                tls::client_config(verifier).expect("client config"),
            ));
            let result = tokio_tungstenite::connect_async_tls_with_config(
                &url,
                None,
                false,
                Some(connector),
            )
            .await;
            (result, seen)
        }
    };

    let (result, seen) = connect(None).await;
    let mut socket = result.expect("tls connect").0;
    let seen_der = seen.lock().unwrap().clone().expect("certificate seen");
    assert_eq!(tls::cert_digest(&seen_der), expected_digest);

    // Enrollment over the encrypted channel, then the proof must bind the
    // exact certificate the TLS layer presented.
    let boot_id = Uuid::new_v4();
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

    let (result, seen) = connect(Some(seen_der.clone().into())).await;
    let mut socket = result.expect("pinned tls connect").0;
    let challenge = receive_challenge(&mut socket).await;
    let hello = signed_hello(&keypair, &challenge, DEDICATED_NODE_ID, Uuid::new_v4());
    let envelope =
        sulion::node_protocol::WireEnvelope::new(DEDICATED_NODE_ID, hello.boot_id, "node.hello");
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
    assert_eq!(proof.public_key, expected_key);
    assert_eq!(
        proof.tls_cert_digest.as_deref(),
        Some(expected_digest.as_str()),
        "the signed proof must bind the TLS certificate",
    );
    let seen_again = seen.lock().unwrap().clone().expect("certificate seen");
    assert_eq!(tls::cert_digest(&seen_again), expected_digest);

    // A node pinned to a different certificate must fail at the TLS layer,
    // before any protocol bytes flow.
    let (other_pem, other_key) = tls::test_certificate();
    let other = tls::ControlTls::from_pem(&other_pem, &other_key).expect("other tls");
    let (result, _) = connect(Some(other.cert_der().clone())).await;
    assert!(
        result.is_err(),
        "a mismatched certificate pin must refuse the connection"
    );
}

#[tokio::test]
async fn http_clients_pin_the_control_certificate_including_the_deployed_ca_flagged_one() {
    // The broker/retrieval path failed in production with CaUsedAsEndEntity:
    // webpki refuses the first certificate generation, which carries CA:TRUE.
    // The HTTP clients therefore pin byte-exactly, like the control channel,
    // and this proves it against a server presenting that exact certificate.
    use sulion::node_protocol::tls;

    let (cert_pem, key_pem) = tls::test_certificate_with_ca_flag();
    let served = tls::ControlTls::from_pem(&cert_pem, &key_pem).expect("parse tls");
    let router =
        axum::Router::new().route("/broker/v1/health", axum::routing::get(|| async { "ok" }));
    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(
        served.server_config().expect("server config"),
    ));
    let handle = axum_server::Handle::new();
    let serve_handle = handle.clone();
    tokio::spawn(async move {
        axum_server::bind_rustls("127.0.0.1:0".parse().unwrap(), rustls_config)
            .handle(serve_handle)
            .serve(router.into_make_service())
            .await
            .expect("serve tls");
    });
    let addr = handle.listening().await.expect("listening");
    let url = format!("https://127.0.0.1:{}/broker/v1/health", addr.port());

    // Default webpki verification refuses this certificate even as a root —
    // the exact production failure.
    let default_client = reqwest::Client::new();
    assert!(
        default_client.get(&url).send().await.is_err(),
        "webpki must refuse the CA-flagged certificate; if this starts \
         passing, the pinned client is no longer load-bearing"
    );

    // The pinned client accepts exactly this certificate...
    let pinned = tls::pinned_http_client(served.cert_der().clone()).expect("pinned client");
    let response = pinned.get(&url).send().await.expect("pinned request");
    assert_eq!(response.status(), 200);

    // ...and only this certificate.
    let (other_pem, other_key) = tls::test_certificate();
    let other = tls::ControlTls::from_pem(&other_pem, &other_key).expect("other tls");
    let wrong_pin = tls::pinned_http_client(other.cert_der().clone()).expect("client");
    assert!(
        wrong_pin.get(&url).send().await.is_err(),
        "a mismatched pin must refuse the connection"
    );
}
