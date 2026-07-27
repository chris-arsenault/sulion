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
    CAPABILITY_OPERATION_PROBE, CONTROL_PROTOCOL_MAX, CONTROL_PROTOCOL_MIN, NODE_PROTOCOL_VERSION,
    PATH_CONTRACT_VERSION,
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
    let state = AppState::new(
        pool,
        "/tmp".into(),
        "/tmp/sulion-workspaces-test".into(),
        "/tmp/sulion-library-test".into(),
        Arc::new(sulion::ingest::Ingester::new()),
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
            serde_json::to_string(&message).expect("hello json").into(),
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
            serde_json::to_string(&NodeWireMessage::Envelope { envelope })
                .expect("heartbeat json")
                .into(),
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
