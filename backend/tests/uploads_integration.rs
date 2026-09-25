#![cfg(feature = "integration-tests")]

mod common;
use axum::{routing::get, Router};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::sync::Arc;
use sulion::{
    db,
    node_protocol::NodeRequestKind,
    uploads::{model::UploadInput, store::StagingStore},
    AppState,
};
use uuid::Uuid;

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (base, task)
}

#[tokio::test]
async fn foreground_upload_validates_metadata_and_installs_only_verified_bytes() {
    let pool = db::connect(&std::env::var("SULION_TEST_DB").expect("test database"))
        .await
        .unwrap();
    db::run_migrations(&pool).await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("repo")).unwrap();
    let state = AppState::new(
        pool.clone(),
        temp.path().into(),
        temp.path().join("workspaces"),
        temp.path().join("library"),
        Arc::new(sulion::ingest::Ingester::new()),
    );
    assert!(state.upload_store.set(StagingStore::for_test()).is_ok());
    let runtime = common::attach_loopback_node(
        &state,
        pool.clone(),
        temp.path(),
        &temp.path().join("workspaces"),
    )
    .await;
    let (base, server) = serve(sulion::app(state.clone())).await;
    let client = reqwest::Client::new();
    let bytes = b"verified uploaded content";
    let checksum = STANDARD.encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref());
    let input = UploadInput {
        repo: Some("repo".into()),
        workspace_id: None,
        directory: "".into(),
        filename: "file.txt".into(),
        size: bytes.len() as i64,
        checksum,
    };
    let response = client
        .post(format!("{base}/api/uploads"))
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let created: Value = response.json().await.unwrap();
    assert_eq!(
        created["grant"]["headers"]["x-amz-meta-upload-binding"],
        input.binding("dev")
    );
    let second: Value = client
        .post(format!("{base}/api/uploads"))
        .json(&input)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(
        second["id"], created["id"],
        "each user attempt is a new transfer"
    );
    assert_eq!(
        client
            .post(format!("{base}/api/uploads"))
            .header("Content-Type", "application/json")
            .body("x".repeat(4097))
            .send()
            .await
            .unwrap()
            .status(),
        413
    );

    for (field, value) in [
        ("size", json!(52428801)),
        ("directory", json!("../outside")),
        ("filename", json!("../escape")),
        ("checksum", json!("invalid")),
    ] {
        let mut invalid = json!(input);
        invalid[field] = value;
        assert_eq!(
            client
                .post(format!("{base}/api/uploads"))
                .json(&invalid)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    // There are no polling/recovery endpoints.
    assert_eq!(
        client
            .get(format!("{base}/api/uploads"))
            .send()
            .await
            .unwrap()
            .status(),
        405
    );

    let (fixture, fixture_server) = serve(
        Router::new()
            .route("/file", get(|| async { "verified uploaded content" }))
            .route("/empty", get(|| async { "" })),
    )
    .await;
    let result = runtime
        .install_upload_for_test(&input, &format!("{fixture}/file"))
        .await
        .unwrap();
    assert_eq!(
        result["path"],
        temp.path().join("repo/file.txt").to_string_lossy().as_ref()
    );
    assert_eq!(result["size"], bytes.len());
    assert_eq!(
        std::fs::read(temp.path().join("repo/file.txt")).unwrap(),
        bytes
    );

    std::fs::write(temp.path().join("repo/file.txt"), b"existing file").unwrap();
    let mut invalid = input.clone();
    invalid.checksum = STANDARD.encode([0; 32]);
    assert!(runtime
        .install_upload_for_test(&invalid, &format!("{fixture}/file"))
        .await
        .is_err());
    invalid = input.clone();
    invalid.size -= 1;
    assert!(runtime
        .install_upload_for_test(&invalid, &format!("{fixture}/file"))
        .await
        .is_err());
    assert_eq!(
        std::fs::read(temp.path().join("repo/file.txt")).unwrap(),
        b"existing file"
    );
    assert!(std::fs::read_dir(temp.path().join("repo"))
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".sulion-upload-")));

    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join("repo/escape")).unwrap();
    invalid = input.clone();
    invalid.directory = "escape".into();
    assert!(runtime
        .install_upload_for_test(&invalid, &format!("{fixture}/file"))
        .await
        .is_err());
    assert!(!outside.path().join("file.txt").exists());

    let workspace_id = Uuid::new_v4();
    let workspace_path = temp.path().join("isolated");
    std::fs::create_dir(&workspace_path).unwrap();
    sqlx::query("INSERT INTO workspaces (id,repo_name,kind,path,node_id) VALUES ($1,'repo','worktree',$2,$3)")
        .bind(workspace_id).bind(workspace_path.to_string_lossy().as_ref()).bind(common::TEST_NODE_ID)
        .execute(&pool).await.unwrap();
    let mut isolated = input.clone();
    isolated.repo = None;
    isolated.workspace_id = Some(workspace_id);
    isolated.directory = ".sulion-paste".into();
    runtime
        .install_upload_for_test(&isolated, &format!("{fixture}/file"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(workspace_path.join(".sulion-paste/file.txt")).unwrap(),
        bytes
    );
    assert!(!temp.path().join("repo/.sulion-paste/file.txt").exists());
    sqlx::query("UPDATE workspaces SET state='deleted' WHERE id=$1")
        .bind(workspace_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(runtime
        .install_upload_for_test(&isolated, &format!("{fixture}/file"))
        .await
        .is_err());

    let mut empty = input.clone();
    empty.size = 0;
    empty.checksum = STANDARD.encode(ring::digest::digest(&ring::digest::SHA256, b"").as_ref());
    runtime
        .install_upload_for_test(&empty, &format!("{fixture}/empty"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("repo/file.txt")).unwrap(),
        b""
    );

    // The production node entry rejects arbitrary URLs before touching the network.
    let result = state
        .node_control
        .request(
            common::TEST_NODE_ID,
            NodeRequestKind::UploadImport,
            json!({"id":Uuid::new_v4(),"input":input,"url":format!("{fixture}/file"),
            "bucket":"sulion-upload-test","region":"us-east-1"}),
        )
        .await;
    assert!(result.is_err());
    fixture_server.abort();
    server.abort();
}
