#![cfg(feature = "integration-tests")]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::{SinkExt, StreamExt};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};
use sulion::node_protocol::model::{
    ControlChallenge, ControlWireMessage, DockerInfo, DockerPolicy, NodeWireMessage,
};
use sulion::node_protocol::{
    heartbeat_envelope, EnrollNodeRequest, NodeControl, NodeHello, NodeOperationKind,
    NodeRequestKind, TerminalEvent, CAPABILITY_OPERATION_PROBE, CONTROL_PROTOCOL_MAX,
    CONTROL_PROTOCOL_MIN, NODE_PROTOCOL_VERSION, PATH_CONTRACT_VERSION,
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

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
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
    start_server_with_node_mode(pool, false).await
}

async fn start_server_with_node_mode(
    pool: db::Pool,
    node_protocol_required: bool,
) -> (String, Arc<AppState>) {
    let state = AppState::new_with_auth_and_node_mode(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
        None,
        node_protocol_required,
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("listener address");
    let router = app(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{addr}"), state)
}

#[tokio::test]
async fn control_health_stays_ready_and_mutations_refuse_local_fallback_without_a_node() {
    let pool = fresh_pool().await;
    let (base, _state) = start_server_with_node_mode(pool, true).await;
    let client = reqwest::Client::new();

    let health = client
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("control health");
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    let health: Value = health.json().await.expect("control health json");
    assert_eq!(health["role"], "control-plane");
    assert_eq!(health["development_node"], "unavailable");

    let app_state = client
        .get(format!("{base}/api/app-state"))
        .send()
        .await
        .expect("control app state");
    assert_eq!(app_state.status(), reqwest::StatusCode::OK);
    let app_state: Value = app_state.json().await.expect("control app state json");
    assert_eq!(app_state["nodes"], json!([]));

    let create = client
        .post(format!("{base}/api/repos"))
        .json(&json!({"name": "must-not-be-local"}))
        .send()
        .await
        .expect("control mutation");
    assert_eq!(create.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let create: Value = create.json().await.expect("control mutation json");
    assert_eq!(create["error"], "development node is unavailable");
}

#[tokio::test]
async fn control_release_identity_is_advertised_and_recorded_for_the_node() {
    let pool = fresh_pool().await;
    let control =
        NodeControl::with_heartbeat_and_release(pool, 5, 20, Some("release-test-123".into()));
    let node_id = Uuid::new_v4();
    control
        .start_loopback(node_id, "release-test")
        .await
        .expect("start release-aware loopback");

    let nodes = control.list_nodes().await.expect("list release-aware node");
    assert_eq!(nodes.len(), 1);
    assert_eq!(
        nodes[0].desired_release_digest.as_deref(),
        Some("release-test-123")
    );
}

fn generate_keypair() -> Ed25519KeyPair {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("generate key");
    Ed25519KeyPair::from_pkcs8(document.as_ref()).expect("parse key")
}

async fn enroll(control: &NodeControl, keypair: &Ed25519KeyPair) -> Uuid {
    let token = control
        .create_enrollment_token("test-node", None, Some(300))
        .await
        .expect("create enrollment token");
    control
        .enroll(EnrollNodeRequest {
            token: token.token,
            public_key: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(keypair.public_key().as_ref()),
        })
        .await
        .expect("enroll")
        .node_id
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
    let message: ControlWireMessage = serde_json::from_str(&text).expect("challenge json");
    match message {
        ControlWireMessage::Challenge { challenge } => challenge,
        _ => panic!("expected challenge"),
    }
}

fn signed_hello(
    keypair: &Ed25519KeyPair,
    challenge: &ControlChallenge,
    node_id: Uuid,
    boot_id: Uuid,
    protocol_version: u32,
) -> NodeHello {
    let mut hello = NodeHello {
        node_id,
        boot_id,
        build_git_sha: "integration-test".into(),
        protocol_version,
        supported_control_min: CONTROL_PROTOCOL_MIN,
        supported_control_max: CONTROL_PROTOCOL_MAX,
        capabilities: vec![CAPABILITY_OPERATION_PROBE.into()],
        docker_policy: DockerPolicy::Direct,
        docker_info: DockerInfo {
            server_version: Some("27.0".into()),
            rootless: true,
        },
        path_contract_version: PATH_CONTRACT_VERSION,
        observed_release_digest: Some("sha256:test".into()),
        signature: String::new(),
    };
    hello.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(keypair.sign(&hello.signing_payload(challenge)).as_ref());
    hello
}

async fn send_hello(
    socket: &mut ClientSocket,
    hello: NodeHello,
) -> tokio_tungstenite::tungstenite::Result<Message> {
    let mut envelope =
        sulion::node_protocol::WireEnvelope::new(hello.node_id, hello.boot_id, "node.hello");
    envelope.protocol_version = hello.protocol_version;
    let message = NodeWireMessage::Hello { envelope, hello };
    socket
        .send(Message::Text(
            serde_json::to_string(&message).expect("hello json"),
        ))
        .await?;
    socket.next().await.expect("handshake response")
}

async fn connect_node(
    http_base: &str,
    keypair: &Ed25519KeyPair,
    node_id: Uuid,
    boot_id: Uuid,
) -> ClientSocket {
    let mut socket = open_socket(http_base).await;
    let challenge = receive_challenge(&mut socket).await;
    let response = send_hello(
        &mut socket,
        signed_hello(keypair, &challenge, node_id, boot_id, NODE_PROTOCOL_VERSION),
    )
    .await
    .expect("handshake response");
    let Message::Text(text) = response else {
        panic!("expected handshake acknowledgment");
    };
    let message: ControlWireMessage = serde_json::from_str(&text).expect("ack json");
    let ControlWireMessage::Envelope { envelope } = message else {
        panic!("expected ack envelope");
    };
    assert_eq!(envelope.message_kind, "control.hello_ack");
    assert_eq!(envelope.payload["accepted"], true);
    socket
}

async fn send_heartbeat(
    socket: &mut ClientSocket,
    node_id: Uuid,
    boot_id: Uuid,
    live_session_ids: Vec<Uuid>,
) {
    let envelope = heartbeat_envelope(node_id, boot_id, live_session_ids, true, None);
    socket
        .send(Message::Text(
            serde_json::to_string(&NodeWireMessage::Envelope { envelope }).expect("heartbeat json"),
        ))
        .await
        .expect("send heartbeat");
    tokio::time::sleep(Duration::from_millis(40)).await;
}

#[tokio::test]
async fn enrollment_rotation_and_revocation_enforce_the_node_credential_lifecycle() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool).await;
    let keypair = generate_keypair();
    let public_key =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(keypair.public_key().as_ref());

    let token_response = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enrollment-tokens"))
        .json(&json!({"display_name": "dell-dev", "ttl_seconds": 300}))
        .send()
        .await
        .expect("token request");
    assert_eq!(token_response.status(), reqwest::StatusCode::CREATED);
    let token: Value = token_response.json().await.expect("token json");
    let enrollment = json!({
        "token": token["token"],
        "public_key": public_key,
    });
    let enrolled_response = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enroll"))
        .json(&enrollment)
        .send()
        .await
        .expect("enroll request");
    assert_eq!(enrolled_response.status(), reqwest::StatusCode::CREATED);
    let enrolled: Value = enrolled_response.json().await.expect("enroll json");
    let node_id: Uuid = enrolled["node_id"]
        .as_str()
        .expect("node id")
        .parse()
        .expect("node uuid");

    let replay = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enroll"))
        .json(&enrollment)
        .send()
        .await
        .expect("replay enrollment");
    assert_eq!(replay.status(), reqwest::StatusCode::UNAUTHORIZED);

    let boot_id = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, node_id, boot_id).await;
    assert_eq!(state.node_control.active_connection_count().await, 1);
    let app_state: Value = reqwest::Client::new()
        .get(format!("{base}/api/app-state"))
        .send()
        .await
        .expect("app state")
        .json()
        .await
        .expect("app state json");
    assert_eq!(app_state["nodes"][0]["id"], node_id.to_string());
    assert_eq!(app_state["nodes"][0]["connection_state"], "connected");

    let rotated_keypair = generate_keypair();
    let rotation_token = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enrollment-tokens"))
        .json(&json!({
            "display_name": "dell-dev",
            "target_node_id": node_id,
            "ttl_seconds": 300,
        }))
        .send()
        .await
        .expect("rotation token");
    assert_eq!(rotation_token.status(), reqwest::StatusCode::CREATED);
    let rotation_token: Value = rotation_token.json().await.expect("rotation token json");
    let rotated = reqwest::Client::new()
        .post(format!("{base}/api/nodes/enroll"))
        .json(&json!({
            "token": rotation_token["token"],
            "public_key": base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(rotated_keypair.public_key().as_ref()),
        }))
        .send()
        .await
        .expect("rotate credential");
    assert_eq!(rotated.status(), reqwest::StatusCode::CREATED);
    let rotated: Value = rotated.json().await.expect("rotation json");
    assert_eq!(rotated["credential_generation"], 2);
    let credential_history: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE replaced_at IS NOT NULL) \
           FROM dev_node_credentials WHERE node_id = $1",
    )
    .bind(node_id)
    .fetch_one(&state.pool)
    .await
    .expect("credential history");
    assert_eq!(credential_history, (2, 1));
    let rotation_close = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("rotation close timeout")
        .expect("rotation close frame")
        .expect("rotation socket read");
    assert!(matches!(rotation_close, Message::Close(_)));

    let rotated_boot_id = Uuid::new_v4();
    let mut socket = connect_node(&base, &rotated_keypair, node_id, rotated_boot_id).await;
    let revoked = reqwest::Client::new()
        .post(format!("{base}/api/nodes/{node_id}/revoke"))
        .send()
        .await
        .expect("revoke");
    assert_eq!(revoked.status(), reqwest::StatusCode::NO_CONTENT);
    let close = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("revocation close timeout")
        .expect("revocation close frame")
        .expect("revocation socket read");
    assert!(matches!(close, Message::Close(_)));
    assert_eq!(state.node_control.active_connection_count().await, 0);
    assert_eq!(
        state.node_control.list_nodes().await.unwrap()[0].connection_state,
        "revoked"
    );
    let revoked_credentials: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dev_node_credentials \
          WHERE node_id = $1 AND revoked_at IS NOT NULL",
    )
    .bind(node_id)
    .fetch_one(&state.pool)
    .await
    .expect("revoked credential history");
    assert_eq!(revoked_credentials, 1);
}

#[tokio::test]
async fn control_restart_reconciliation_preserves_node_sessions_and_same_boot_reconnects() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone()).await;
    let keypair = generate_keypair();
    let node_id = enroll(&state.node_control, &keypair).await;
    let boot_id = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, node_id, boot_id).await;
    let session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions \
            (id, repo, working_dir, state, node_id, node_boot_id) \
         VALUES ($1, 'repo', '/repo', 'live', $2, $3)",
    )
    .bind(session_id)
    .bind(node_id)
    .bind(boot_id)
    .execute(&pool)
    .await
    .unwrap();
    send_heartbeat(&mut socket, node_id, boot_id, vec![session_id]).await;

    let reconciled = sulion::pty::reconcile_orphans_on_startup(&pool)
        .await
        .expect("control startup reconciliation");
    assert_eq!(reconciled, 0);
    let state_after_restart: String =
        sqlx::query_scalar("SELECT state FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state_after_restart, "live");

    socket.close(None).await.expect("close first connection");
    let mut reconnected = connect_node(&base, &keypair, node_id, boot_id).await;
    send_heartbeat(&mut reconnected, node_id, boot_id, vec![session_id]).await;
    let session_state: String = sqlx::query_scalar("SELECT state FROM pty_sessions WHERE id = $1")
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(session_state, "live");
}

#[tokio::test]
async fn loopback_replays_duplicate_operations_without_duplicate_effects() {
    let pool = fresh_pool().await;
    let control = NodeControl::new(pool);
    let node_id = Uuid::new_v4();
    control
        .start_loopback(node_id, "standalone-test")
        .await
        .expect("start loopback");

    let first = control
        .request_operation(
            node_id,
            "probe:stable-request",
            NodeOperationKind::ProbeEcho,
            None,
            json!({"value": 42}),
        )
        .await
        .expect("first operation");
    let completed = wait_for_operation(&control, first.operation_id).await;
    assert_eq!(completed.status, "succeeded");
    assert_eq!(completed.result, Some(json!({"echo": {"value": 42}})));

    let duplicate = control
        .request_operation(
            node_id,
            "probe:stable-request",
            NodeOperationKind::ProbeEcho,
            None,
            json!({"value": 42}),
        )
        .await
        .expect("duplicate operation");
    assert_eq!(duplicate.operation_id, first.operation_id);
    assert_eq!(duplicate.status, "succeeded");

    let conflict = control
        .request_operation(
            node_id,
            "probe:stable-request",
            NodeOperationKind::ProbeEcho,
            None,
            json!({"value": 43}),
        )
        .await;
    assert!(matches!(
        conflict,
        Err(sulion::node_protocol::NodeProtocolError::IdempotencyConflict)
    ));
}

#[tokio::test]
async fn extracted_runtime_preserves_a_pty_and_snapshot_across_control_replacement() {
    let pool = fresh_pool().await;
    let root = tempfile::tempdir().expect("runtime root");
    let repos_root = root.path().join("repos");
    let workspaces_root = root.path().join("workspaces");
    std::fs::create_dir_all(&repos_root).expect("repos root");
    std::fs::create_dir_all(&workspaces_root).expect("workspaces root");
    let node_id = Uuid::new_v4();
    let boot_id = Uuid::new_v4();
    let runtime = NodeRuntime::new(node_id, boot_id, pool.clone(), repos_root, workspaces_root);
    let first_control = NodeControl::new(pool.clone());
    first_control
        .start_runtime_loopback(runtime.clone(), "runtime-test")
        .await
        .expect("connect extracted runtime");

    first_control
        .request_operation_and_wait(
            node_id,
            "repo:create:restart-test",
            NodeOperationKind::RepoCreate,
            None,
            serde_json::to_value(RepoCreateRequest {
                name: "restart-test".into(),
                git_url: None,
            })
            .unwrap(),
        )
        .await
        .expect("create repo through node");

    let session_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    first_control
        .request_operation_and_wait(
            node_id,
            &format!("session:create:{session_id}"),
            NodeOperationKind::SessionCreate,
            Some(session_id),
            serde_json::to_value(SessionCreateRequest {
                session_id,
                allocated_workspace_id: workspace_id,
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
        .expect("create session through node");

    let first_attachment = first_control
        .open_terminal(node_id, session_id)
        .await
        .expect("first terminal attach");
    let (first_sender, mut first_events) = first_attachment.into_parts();
    wait_for_terminal_ready(&mut first_events).await;
    first_sender
        .send_input(b"printf 'CONTROL_RESTART_SENTINEL\\n'\r")
        .await
        .expect("write terminal marker");
    wait_for_terminal_text(&mut first_events, b"CONTROL_RESTART_SENTINEL").await;
    first_sender.close().await;

    let replacement_control = NodeControl::new(pool.clone());
    replacement_control
        .start_runtime_loopback(runtime.clone(), "runtime-test")
        .await
        .expect("reconnect runtime to replacement control");
    assert_eq!(runtime.pty().live_count().await, 1);

    let replacement_attachment = replacement_control
        .open_terminal(node_id, session_id)
        .await
        .expect("replacement terminal attach");
    let (replacement_sender, mut replacement_events) = replacement_attachment.into_parts();
    wait_for_terminal_text(&mut replacement_events, b"CONTROL_RESTART_SENTINEL").await;
    replacement_sender.close().await;

    replacement_control
        .request_operation_and_wait(
            node_id,
            &format!("session:delete:{session_id}"),
            NodeOperationKind::SessionDelete,
            Some(session_id),
            serde_json::to_value(ResourceRequest { id: session_id }).unwrap(),
        )
        .await
        .expect("delete node session");
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
        .request_operation_and_wait(
            node_id,
            "repo:create:filesystem-test",
            NodeOperationKind::RepoCreate,
            None,
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
                None,
                serde_json::to_value(RepoPathRequest {
                    repo: "filesystem-test".into(),
                    path: Some(path.into()),
                    all: false,
                })
                .unwrap(),
            )
            .await
            .expect_err("escaped path must fail");
        assert!(
            matches!(
                error,
                sulion::node_protocol::NodeProtocolError::Remote { .. }
            ),
            "unexpected error for {path}: {error}"
        );
    }
}

#[tokio::test]
async fn heartbeat_expiry_marks_connectivity_stale_without_killing_sessions() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone()).await;
    let keypair = generate_keypair();
    let node_id = enroll(&state.node_control, &keypair).await;
    let boot_id = Uuid::new_v4();
    let mut socket = connect_node(&base, &keypair, node_id, boot_id).await;
    let session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions \
            (id, repo, working_dir, state, node_id, node_boot_id) \
         VALUES ($1, 'repo', '/repo', 'live', $2, $3)",
    )
    .bind(session_id)
    .bind(node_id)
    .bind(boot_id)
    .execute(&pool)
    .await
    .unwrap();
    send_heartbeat(&mut socket, node_id, boot_id, vec![session_id]).await;

    let expired = state
        .node_control
        .expire_heartbeats_at(chrono::Utc::now() + chrono::Duration::seconds(30))
        .await
        .expect("expire heartbeat");
    assert_eq!(expired, 1);
    let row: (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT state, node_disconnected_at FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, "live");
    assert!(row.1.is_some());
    assert_eq!(
        state.node_control.list_nodes().await.unwrap()[0].connection_state,
        "stale"
    );
}

#[tokio::test]
async fn a_new_boot_ends_only_sessions_owned_by_the_prior_boot() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool.clone()).await;
    let keypair = generate_keypair();
    let node_id = enroll(&state.node_control, &keypair).await;
    let first_boot = Uuid::new_v4();
    let _first_socket = connect_node(&base, &keypair, node_id, first_boot).await;
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

    let second_boot = Uuid::new_v4();
    let _second_socket = connect_node(&base, &keypair, node_id, second_boot).await;
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
async fn incompatible_protocol_is_visible_but_never_becomes_active() {
    let pool = fresh_pool().await;
    let (base, state) = start_server(pool).await;
    let keypair = generate_keypair();
    let node_id = enroll(&state.node_control, &keypair).await;
    let boot_id = Uuid::new_v4();
    let mut socket = open_socket(&base).await;
    let challenge = receive_challenge(&mut socket).await;
    let response = send_hello(
        &mut socket,
        signed_hello(
            &keypair,
            &challenge,
            node_id,
            boot_id,
            NODE_PROTOCOL_VERSION + 1,
        ),
    )
    .await
    .expect("incompatible ack");
    let Message::Text(text) = response else {
        panic!("expected incompatibility acknowledgment");
    };
    let message: ControlWireMessage = serde_json::from_str(&text).expect("ack json");
    let ControlWireMessage::Envelope { envelope } = message else {
        panic!("expected ack envelope");
    };
    assert_eq!(envelope.payload["accepted"], false);
    assert_eq!(
        envelope.payload["reason_code"],
        Value::String("node_protocol_version".into())
    );
    assert_eq!(state.node_control.active_connection_count().await, 0);
    let node = state.node_control.list_nodes().await.unwrap().remove(0);
    assert_eq!(node.connection_state, "incompatible");
    assert_eq!(
        node.compatibility_error.as_deref(),
        Some("node_protocol_version")
    );
}

async fn wait_for_operation(
    control: &NodeControl,
    operation_id: Uuid,
) -> sulion::node_protocol::NodeOperationView {
    for _ in 0..50 {
        let operation = control
            .operation(operation_id)
            .await
            .expect("load operation")
            .expect("operation exists");
        if matches!(
            operation.status.as_str(),
            "succeeded" | "failed" | "canceled"
        ) {
            return operation;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("operation did not complete");
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

async fn wait_for_terminal_text(
    events: &mut tokio::sync::mpsc::Receiver<TerminalEvent>,
    needle: &[u8],
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("terminal output timeout")
            .expect("terminal event stream closed")
        {
            TerminalEvent::Snapshot(chunk) | TerminalEvent::Output(chunk) => {
                bytes.extend(chunk);
                if bytes.windows(needle.len()).any(|window| window == needle) {
                    return;
                }
            }
            TerminalEvent::Ready => {}
            TerminalEvent::Dead(code) => panic!("terminal died before marker: {code:?}"),
            TerminalEvent::Disconnected => panic!("terminal disconnected before marker"),
        }
    }
}
