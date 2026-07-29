#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use sulion::{app, db, AppState};
use tokio::net::TcpListener;
use uuid::Uuid;

mod common;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE retrieval_embedding_backfills, retrieval_embedding_sources, retrieval_embeddings, \
         plan_events, plan_attachments, plan_phases, plans, session_activity_state, \
         events, ingester_state, claude_sessions, pty_sessions, repos, \
         repo_runtime_state, repo_dirty_paths, timeline_session_state, \
         future_prompt_session_state, workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("truncate test tables");
    pool
}

struct Harness {
    base: String,
    state: Arc<AppState>,
    client: reqwest::Client,
    _tmp: tempfile::TempDir,
}

impl Harness {
    async fn new() -> Self {
        let pool = fresh_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let repos_root = tmp.path().join("repos");
        let workspaces_root = tmp.path().join("workspaces");
        std::fs::create_dir_all(&repos_root).unwrap();
        std::fs::create_dir_all(&workspaces_root).unwrap();
        let (state, _runtime) = common::state_with_loopback_node(
            pool,
            &repos_root,
            &workspaces_root,
            &tmp.path().join("library"),
        )
        .await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = app(state.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            base: format!("http://{addr}"),
            state,
            client: reqwest::Client::new(),
            _tmp: tmp,
        }
    }

    async fn shutdown_sessions(&self) {
        common::shutdown_node_sessions(&self.state).await;
    }
}

#[tokio::test]
async fn isolated_session_creates_git_worktree_workspace() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "isolated" }),
    )
    .await;

    let session_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace = created["workspace"].as_object().unwrap();
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace_path = PathBuf::from(workspace["path"].as_str().unwrap());
    assert_eq!(workspace["kind"], "worktree");
    assert!(workspace_path.starts_with(&h.state.workspaces_root));
    assert_eq!(
        git_stdout(&workspace_path, &["branch", "--show-current"]).trim(),
        workspace["branch_name"].as_str().unwrap(),
    );
    assert_ne!(workspace_path, repo_path);

    std::fs::write(workspace_path.join("agent.txt"), "changed\n").unwrap();
    h.state
        .workspace_state
        .request_refresh(workspace_id)
        .await
        .unwrap();
    h.state.workspace_state.reconcile_due_once(4).await.unwrap();

    let dirty: serde_json::Value = h
        .client
        .get(format!(
            "{}/api/workspaces/{}/dirty-paths",
            h.base, workspace_id
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(dirty["workspace_id"], workspace_id.to_string());
    assert_eq!(dirty["dirty_by_path"]["agent.txt"], "??");

    common::delete_node_session(&h.state, session_id).await;
}

#[tokio::test]
async fn delete_workspace_removes_worktree_branch_and_row() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "isolated" }),
    )
    .await;

    let session_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace = created["workspace"].as_object().unwrap();
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace_path = PathBuf::from(workspace["path"].as_str().unwrap());
    let branch_name = workspace["branch_name"].as_str().unwrap().to_string();

    common::delete_node_session(&h.state, session_id).await;
    let resp = h
        .client
        .delete(format!("{}/api/workspaces/{workspace_id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(!workspace_path.exists());
    assert_eq!(
        git_stdout(&repo_path, &["branch", "--list", &branch_name]),
        ""
    );
    assert!(h
        .state
        .workspace_state
        .load_workspace(workspace_id)
        .await
        .is_err());
}

#[tokio::test]
async fn delete_workspace_rejects_unmerged_branch_commits_without_force() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "isolated" }),
    )
    .await;

    let session_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace = created["workspace"].as_object().unwrap();
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace_path = PathBuf::from(workspace["path"].as_str().unwrap());

    std::fs::write(workspace_path.join("agent.txt"), "changed\n").unwrap();
    run(&workspace_path, &["add", "agent.txt"]);
    run(&workspace_path, &["commit", "-m", "agent work"]);

    common::delete_node_session(&h.state, session_id).await;
    let resp = h
        .client
        .delete(format!("{}/api/workspaces/{workspace_id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("not merged"));

    let resp = h
        .client
        .delete(format!(
            "{}/api/workspaces/{workspace_id}?force=true",
            h.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(!workspace_path.exists());
}

#[tokio::test]
async fn delete_workspace_allows_branch_commits_merged_into_target() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "isolated" }),
    )
    .await;

    let session_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace = created["workspace"].as_object().unwrap();
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace_path = PathBuf::from(workspace["path"].as_str().unwrap());
    let branch_name = workspace["branch_name"].as_str().unwrap().to_string();

    std::fs::write(workspace_path.join("agent.txt"), "changed\n").unwrap();
    run(&workspace_path, &["add", "agent.txt"]);
    run(&workspace_path, &["commit", "-m", "agent work"]);
    run(&repo_path, &["merge", &branch_name]);

    common::delete_node_session(&h.state, session_id).await;
    let resp = h
        .client
        .delete(format!("{}/api/workspaces/{workspace_id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(!workspace_path.exists());
    assert_eq!(
        git_stdout(&repo_path, &["branch", "--list", &branch_name]),
        ""
    );
}

#[tokio::test]
async fn delete_workspace_rejects_live_sessions_and_dirty_worktrees() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "isolated" }),
    )
    .await;

    let session_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace = created["workspace"].as_object().unwrap();
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace_path = PathBuf::from(workspace["path"].as_str().unwrap());

    let resp = h
        .client
        .delete(format!("{}/api/workspaces/{workspace_id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("live or orphaned"));

    common::delete_node_session(&h.state, session_id).await;
    std::fs::write(workspace_path.join("agent.txt"), "changed\n").unwrap();

    let resp = h
        .client
        .delete(format!("{}/api/workspaces/{workspace_id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("uncommitted"));

    let resp = h
        .client
        .delete(format!(
            "{}/api/workspaces/{workspace_id}?force=true",
            h.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(!workspace_path.exists());
}

#[tokio::test]
async fn delete_workspace_removes_missing_worktree_registration() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "isolated" }),
    )
    .await;

    let session_id = created["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace = created["workspace"].as_object().unwrap();
    let workspace_id = workspace["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let workspace_path = PathBuf::from(workspace["path"].as_str().unwrap());
    let branch_name = workspace["branch_name"].as_str().unwrap().to_string();

    common::delete_node_session(&h.state, session_id).await;
    std::fs::remove_dir_all(&workspace_path).unwrap();

    let resp = h
        .client
        .delete(format!("{}/api/workspaces/{workspace_id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NO_CONTENT);
    assert_eq!(
        git_stdout(&repo_path, &["branch", "--list", &branch_name]),
        ""
    );
    assert!(
        !git_stdout(&repo_path, &["worktree", "list", "--porcelain"])
            .contains(workspace_path.to_str().unwrap())
    );
}

#[tokio::test]
async fn main_session_binds_canonical_repo_workspace() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({ "repo": "app", "workspace_mode": "main" }),
    )
    .await;

    let workspace = created["workspace"].as_object().unwrap();
    assert_eq!(workspace["kind"], "main");
    assert_eq!(
        PathBuf::from(workspace["path"].as_str().unwrap()),
        repo_path
    );
    assert_eq!(
        created["working_dir"].as_str().unwrap(),
        repo_path.to_str().unwrap()
    );

    h.shutdown_sessions().await;
}

#[tokio::test]
async fn resume_with_working_dir_defaults_to_main_workspace() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);
    std::fs::remove_dir_all(&h.state.workspaces_root).unwrap();
    std::fs::write(&h.state.workspaces_root, "not a directory").unwrap();

    let resp = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({
            "repo": "app",
            "working_dir": repo_path,
            "resume_session_uuid": Uuid::new_v4(),
            "resume_agent": "claude-code"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::CREATED);
    let created: serde_json::Value = resp.json().await.unwrap();

    let workspace = created["workspace"].as_object().unwrap();
    assert_eq!(workspace["kind"], "main");
    assert_eq!(
        PathBuf::from(workspace["path"].as_str().unwrap()),
        repo_path
    );
    assert_eq!(
        created["working_dir"].as_str().unwrap(),
        repo_path.to_str().unwrap()
    );

    h.shutdown_sessions().await;
}

#[tokio::test]
async fn isolated_session_rejects_working_dir_before_worktree_creation() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);
    std::fs::remove_dir_all(&h.state.workspaces_root).unwrap();
    std::fs::write(&h.state.workspaces_root, "not a directory").unwrap();

    let resp = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({
            "repo": "app",
            "workspace_mode": "isolated",
            "working_dir": repo_path
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"],
        "working_dir is only supported with workspace_mode=main"
    );
}

fn init_git_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    run(path, &["init", "-b", "main"]);
    run(path, &["config", "user.email", "sulion@example.invalid"]);
    run(path, &["config", "user.name", "Sulion Test"]);
    std::fs::write(path.join("README.md"), "# app\n").unwrap();
    run(path, &["add", "README.md"]);
    run(path, &["commit", "-m", "initial"]);
}

fn run(path: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_stdout(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
