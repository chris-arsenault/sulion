#![cfg(feature = "integration-tests")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
        "TRUNCATE meta_repo_members, meta_repos, \
         retrieval_embedding_backfills, retrieval_embedding_sources, retrieval_embeddings, \
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

async fn create_meta_repo(h: &Harness, name: &str, members: &[&str]) -> serde_json::Value {
    h.state.repo_state.sync_repos_once().await.unwrap();
    let response = h
        .client
        .post(format!("{}/api/meta-repos", h.base))
        .json(&json!({
            "name": name,
            "members": members,
            "primary_repo_name": members.first()
        }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, reqwest::StatusCode::CREATED, "{body}");
    body
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

#[tokio::test]
async fn main_collection_session_uses_only_the_primary_workspace() {
    let h = Harness::new().await;
    let alpha_path = h.state.repos_root.join("alpha");
    let beta_path = h.state.repos_root.join("beta");
    init_git_repo(&alpha_path);
    init_git_repo(&beta_path);
    let group = create_meta_repo(&h, "Platform", &["alpha", "beta"]).await;

    let created = common::create_session(
        &h.client,
        &h.base,
        json!({
            "meta_repo_id": group["id"],
            "workspace_mode": "main"
        }),
    )
    .await;

    assert_eq!(created["repo"], "alpha");
    assert_eq!(created["meta_repo"]["name"], "Platform");
    assert_eq!(created["workspace"]["repo_name"], "alpha");
    assert_eq!(created["workspace"]["kind"], "main");
    assert!(created.get("repositories").is_none());
    let stored_group_id: Option<Uuid> =
        sqlx::query_scalar("SELECT meta_repo_id FROM pty_sessions WHERE id = $1")
            .bind(created["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(stored_group_id, group["id"].as_str().unwrap().parse().ok());

    h.shutdown_sessions().await;
}

#[tokio::test]
async fn isolated_collection_is_rejected_before_worktree_creation() {
    let h = Harness::new().await;
    let alpha_path = h.state.repos_root.join("alpha");
    init_git_repo(&alpha_path);
    std::fs::create_dir_all(h.state.repos_root.join("beta")).unwrap();
    let group = create_meta_repo(&h, "Platform", &["alpha", "beta"]).await;

    let response = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({
            "meta_repo_id": group["id"],
            "workspace_mode": "isolated"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("workspace_mode=main"),
        "{body}"
    );

    let active_worktrees: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspaces WHERE kind = 'worktree' AND state <> 'deleted'",
    )
    .fetch_one(&h.state.pool)
    .await
    .unwrap();
    assert_eq!(active_worktrees, 0);
}

/// Row identities of a table's dirty-path rows: a rewrite gives every row
/// a new xmin even when the content is identical, so unchanged xmins
/// prove the poller left the rows alone. The harness node runs its own
/// reconcile loop, which can process the same row alongside the test's
/// explicit call, so the identities are read twice and only returned once
/// stable.
async fn dirty_path_xmins(pool: &db::Pool, table: &str, key: &str, id: &str) -> Vec<(String, i64)> {
    let read = || async {
        sqlx::query_as::<_, (String, i64)>(&format!(
            "SELECT path, xmin::text::bigint FROM {table} WHERE {key} = $1::text::{} ORDER BY path",
            if table == "workspace_dirty_paths" {
                "uuid"
            } else {
                "text"
            }
        ))
        .bind(id)
        .fetch_all(pool)
        .await
        .unwrap()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut last = read().await;
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let next = read().await;
        if next == last {
            return next;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "dirty-path rows never settled"
        );
        last = next;
    }
}

#[tokio::test]
async fn repo_poller_writes_only_when_git_status_changes() {
    let h = Harness::new().await;
    let repo_path = h.state.repos_root.join("quiet");
    init_git_repo(&repo_path);
    std::fs::write(repo_path.join("scratch.txt"), "draft\n").unwrap();
    h.state.repo_state.sync_repos_once().await.unwrap();
    h.state.repo_state.reconcile_due_once(4).await.unwrap();

    let before = dirty_path_xmins(&h.state.pool, "repo_dirty_paths", "repo_name", "quiet").await;
    assert_eq!(before.len(), 1, "scratch.txt is untracked: {before:?}");
    let (started_before, revision_before): (Option<chrono::DateTime<chrono::Utc>>, i64) =
        sqlx::query_as(
            "SELECT status_started_at, git_revision FROM repo_runtime_state WHERE repo_name = 'quiet'",
        )
        .fetch_one(&h.state.pool)
        .await
        .unwrap();

    // Nothing changed: the cycle reschedules and leaves every row alone.
    h.state.repo_state.request_refresh("quiet").await.unwrap();
    h.state.repo_state.reconcile_due_once(4).await.unwrap();
    let after = dirty_path_xmins(&h.state.pool, "repo_dirty_paths", "repo_name", "quiet").await;
    assert_eq!(after, before, "unchanged status rewrote dirty-path rows");
    let (started_after, revision_after, next_due_in_future): (
        Option<chrono::DateTime<chrono::Utc>>,
        i64,
        bool,
    ) = sqlx::query_as(
        "SELECT status_started_at, git_revision, next_status_at > NOW() \
           FROM repo_runtime_state WHERE repo_name = 'quiet'",
    )
    .fetch_one(&h.state.pool)
    .await
    .unwrap();
    assert_eq!(started_after, started_before);
    assert_eq!(revision_after, revision_before);
    assert!(next_due_in_future, "unchanged repo was not rescheduled");

    // A change rewrites the rows and bumps the revision.
    std::fs::write(repo_path.join("scratch.txt"), "draft two\n").unwrap();
    std::fs::write(repo_path.join("more.txt"), "x\n").unwrap();
    h.state.repo_state.request_refresh("quiet").await.unwrap();
    h.state.repo_state.reconcile_due_once(4).await.unwrap();
    let changed = dirty_path_xmins(&h.state.pool, "repo_dirty_paths", "repo_name", "quiet").await;
    assert_eq!(changed.len(), 2);
    assert!(changed
        .iter()
        .all(|(_, xmin)| before.iter().all(|(_, b)| b != xmin)));
    let (revision_changed,): (i64,) =
        sqlx::query_as("SELECT git_revision FROM repo_runtime_state WHERE repo_name = 'quiet'")
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(revision_changed, revision_before + 1);
}

#[tokio::test]
async fn workspace_poller_writes_only_when_git_status_changes() {
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
    let workspace_id = created["workspace"]["id"].as_str().unwrap().to_string();
    let workspace_path = PathBuf::from(created["workspace"]["path"].as_str().unwrap());
    std::fs::write(workspace_path.join("agent.txt"), "changed\n").unwrap();
    let id: Uuid = workspace_id.parse().unwrap();
    h.state.workspace_state.request_refresh(id).await.unwrap();
    h.state.workspace_state.reconcile_due_once(4).await.unwrap();

    let before = dirty_path_xmins(
        &h.state.pool,
        "workspace_dirty_paths",
        "workspace_id",
        &workspace_id,
    )
    .await;
    assert_eq!(before.len(), 1, "{before:?}");
    h.state.workspace_state.request_refresh(id).await.unwrap();
    h.state.workspace_state.reconcile_due_once(4).await.unwrap();
    let after = dirty_path_xmins(
        &h.state.pool,
        "workspace_dirty_paths",
        "workspace_id",
        &workspace_id,
    )
    .await;
    assert_eq!(after, before, "unchanged status rewrote dirty-path rows");
    let (rescheduled,): (bool,) =
        sqlx::query_as("SELECT next_status_at > NOW() FROM workspaces WHERE id = $1")
            .bind(id)
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert!(rescheduled);

    // An untracked file's content is not part of the status fingerprint;
    // a new path is.
    std::fs::write(workspace_path.join("second.txt"), "x\n").unwrap();
    h.state.workspace_state.request_refresh(id).await.unwrap();
    h.state.workspace_state.reconcile_due_once(4).await.unwrap();
    let changed = dirty_path_xmins(
        &h.state.pool,
        "workspace_dirty_paths",
        "workspace_id",
        &workspace_id,
    )
    .await;
    assert_ne!(
        changed, before,
        "changed status did not rewrite dirty-path rows"
    );

    common::delete_node_session(&h.state, session_id).await;
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
