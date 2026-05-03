#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use sulion::{app, db, AppState};
use tokio::net::TcpListener;
use uuid::Uuid;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE events, ingester_state, claude_sessions, pty_sessions, repos, \
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
        let state = AppState::new(
            pool,
            repos_root,
            workspaces_root,
            tmp.path().join("library"),
            Arc::new(sulion::ingest::Ingester::new()),
        );
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
        if let Ok(list) = self.state.pty.list().await {
            for meta in list {
                let _ = self.state.pty.delete(meta.id).await;
            }
        }
    }
}

#[tokio::test]
async fn isolated_session_creates_git_worktree_workspace() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created: serde_json::Value = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "app", "workspace_mode": "isolated" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

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

    h.state.pty.delete(session_id).await.unwrap();
}

#[tokio::test]
async fn main_session_binds_canonical_repo_workspace() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("app");
    init_git_repo(&repo_path);

    let created: serde_json::Value = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "repo": "app", "workspace_mode": "main" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

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
