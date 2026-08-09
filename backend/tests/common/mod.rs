//! Shared setup for the Postgres-backed integration suites.
//!
//! Every deployment routes local mutations and terminal attachment through the
//! node protocol; standalone differs only in that its node runs in-process over
//! the loopback transport. Tests therefore build the same shape rather than a
//! bare `AppState`, so what they exercise is what ships.

// Each integration target includes this module and uses part of it.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use sulion::node_runtime::NodeRuntime;
use sulion::{db, AppState};

/// Node identity for in-process test nodes. Matches the standalone default in
/// `StandaloneNodeConfig`, which is likewise a fixed id rather than a paired one.
pub const TEST_NODE_ID: uuid::Uuid = uuid::Uuid::from_u128(1);

/// Builds an `AppState` with an in-process node attached over loopback, the way
/// `main.rs` wires standalone. The runtime is given the same roots as the state
/// so paths resolve identically on both sides of the protocol.
pub async fn state_with_loopback_node(
    pool: db::Pool,
    repos_root: &Path,
    workspaces_root: &Path,
    library_root: &Path,
) -> (Arc<AppState>, Arc<NodeRuntime>) {
    let state = AppState::new(
        pool.clone(),
        repos_root.to_path_buf(),
        workspaces_root.to_path_buf(),
        library_root.to_path_buf(),
        Arc::new(sulion::ingest::Ingester::new()),
    );
    let runtime = attach_loopback_node(&state, pool, repos_root, workspaces_root).await;
    (state, runtime)
}

/// Creates a session and fails with the server's message if it did not.
///
/// Unwrapping a field of the response directly reports "unwrap on None" and
/// discards the error that explains why. That has cost several rounds of
/// re-running a suite by hand to find out what a 503 actually said.
pub async fn create_session(
    client: &reqwest::Client,
    base: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let response = client
        .post(format!("{base}/api/sessions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let created: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        status,
        reqwest::StatusCode::CREATED,
        "create session {body} failed: {created}"
    );
    created
}

/// Kills one PTY through the node that owns it.
///
/// `state.pty.delete` does not: the control plane's manager has no handle on a
/// node-spawned process, so it marks the row deleted and leaves the shell
/// running — with its working directory still inside the workspace a test is
/// about to remove.
pub async fn delete_node_session(state: &Arc<AppState>, session_id: uuid::Uuid) {
    let node_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT node_id FROM pty_sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
    let Some(node_id) = node_id else { return };
    let _ = state
        .node_control
        .request(
            node_id,
            sulion::node_protocol::NodeRequestKind::SessionDelete,
            serde_json::json!({ "id": session_id }),
        )
        .await;
}

/// Kills every live PTY the node owns.
///
/// Tests that cleaned up through `state.pty` were deleting nothing: the control
/// plane's manager does not own node-spawned PTYs, so the shells survived the
/// test. A surviving child inherits the test binary's stdout, which keeps the
/// pipe open after the last test finishes and makes the runner look wedged when
/// it is merely waiting on a process that will never exit. Deleting through the
/// node is what the API does, and it actually reaps the process.
pub async fn shutdown_node_sessions(state: &Arc<AppState>) {
    let live: Vec<(uuid::Uuid, uuid::Uuid)> = sqlx::query_as(
        "SELECT id, node_id FROM pty_sessions WHERE state = 'live' AND node_id IS NOT NULL",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    for (session_id, node_id) in live {
        let payload = serde_json::json!({ "id": session_id });
        let _ = state
            .node_control
            .request(
                node_id,
                sulion::node_protocol::NodeRequestKind::SessionDelete,
                payload,
            )
            .await;
    }
}

/// Attaches an in-process node to an existing state and hands back the runtime.
///
/// Suites that need to place a PTY the websocket layer can attach to must spawn
/// it through this runtime's manager: terminal attach is served by the node that
/// owns the process, so a PTY spawned on the control plane's manager has no
/// owner and the ticket route answers 503.
pub async fn attach_loopback_node(
    state: &Arc<AppState>,
    pool: db::Pool,
    repos_root: &Path,
    workspaces_root: &Path,
) -> Arc<NodeRuntime> {
    let (link, events) = in_process_devenv().await;
    let runtime = NodeRuntime::new(
        TEST_NODE_ID,
        uuid::Uuid::new_v4(),
        pool,
        repos_root.to_path_buf(),
        workspaces_root.to_path_buf(),
        link,
        events,
    );
    state
        .node_control
        .start_runtime_loopback(runtime.clone(), "integration-test-node")
        .await
        .expect("start loopback node");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    while !state
        .node_control
        .supports_capability(
            TEST_NODE_ID,
            sulion::node_protocol::MULTI_REPO_SESSION_CAPABILITY,
        )
        .await
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "loopback node did not report runtime capabilities"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    runtime.clone().run_background_managers().await;
    runtime
}

/// An in-process devenv: a `DevenvServer` wired to a `DevenvLink` over a
/// duplex stream — the production wire path minus the unix socket, so tests
/// exercise exactly the protocol the deployed node speaks.
pub async fn in_process_devenv() -> (
    Arc<sulion::devenv::link::DevenvLink>,
    tokio::sync::mpsc::UnboundedReceiver<sulion::devenv::link::LinkEvent>,
) {
    let (link, events) = sulion::devenv::link::DevenvLink::new();
    let server = Arc::new(sulion::devenv::server::DevenvServer::new());
    let (node_side, devenv_side) = tokio::io::duplex(1024 * 1024);
    tokio::spawn(server.clone().serve(devenv_side));
    tokio::spawn(link.clone().handle_connection(node_side));
    while !link.connected().await {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    (link, events)
}

/// A PTY manager backed by an in-process devenv, for suites that drive the
/// manager directly rather than through a runtime.
pub async fn devenv_backed_pty_manager(pool: db::Pool) -> Arc<sulion::pty::PtyManager> {
    let (link, events) = in_process_devenv().await;
    sulion::pty::PtyManager::with_devenv(pool, link, events)
}
