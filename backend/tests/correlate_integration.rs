#![cfg(feature = "integration-tests")]

//! Correlation socket integration tests. Exercise the SessionStart-hook
//! path end-to-end: bind a socket, write a JSON line, verify the DB rows.

use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use sulion::activity::ActivityState;
use sulion::codex::{run_launcher, LauncherConfig};
use sulion::correlate::{self, ControlRequest, CorrelateMsg, RuntimeEvent, RuntimeMsg};
use sulion::db;
use sulion::plans::{NewPhase, UpdatePhaseInput};
use sulion::pty::SpawnParams;

mod common;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    sqlx::query(
        "TRUNCATE retrieval_embedding_backfills, retrieval_embedding_sources, retrieval_embeddings, \
         plan_events, plan_attachments, plan_phases, plans, session_activity_state, \
         events, ingester_state, claude_sessions, pty_sessions, repos, \
         workspaces, workspace_dirty_paths RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .ok();
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

fn tmp_sock() -> PathBuf {
    std::env::temp_dir().join(format!("sulion-corr-{}.sock", Uuid::new_v4()))
}

async fn wait_for_socket(path: &Path) {
    for _ in 0..50 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("socket never created: {}", path.display());
}

fn write_fake_codex(path: &Path) {
    std::fs::write(
        path,
        "#!/usr/bin/env bash\nset -euo pipefail\nexec 3>>\"$1\"\nprintf '{\"kind\":\"response_item\"}\\n' >&3\nsleep 0.6\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

fn write_fake_codex_with_nested_child(path: &Path) {
    std::fs::write(
        path,
        "#!/usr/bin/env bash\nset -euo pipefail\nchild_rollout=\"$1\"\nroot_rollout=\"$2\"\n(\n  exec 4>>\"${child_rollout}\"\n  printf '{\"kind\":\"response_item\",\"who\":\"child\"}\\n' >&4\n  sleep 1\n) &\nsleep 0.25\nexec 3>>\"${root_rollout}\"\nprintf '{\"kind\":\"response_item\",\"who\":\"root\"}\\n' >&3\nwait\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

#[tokio::test]
async fn apply_upserts_claude_session_and_points_pty() {
    let pool = fresh_pool().await;
    let mgr = common::devenv_backed_pty_manager(pool.clone()).await;

    // Create a real PTY so we have a live row to point at.
    let pty = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .expect("spawn");

    let claude_uuid = Uuid::new_v4();
    correlate::apply(
        &pool,
        &CorrelateMsg {
            pty_id: pty.id,
            session_uuid: claude_uuid,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .expect("apply");

    let (pty_link,): (Option<Uuid>,) =
        sqlx::query_as("SELECT pty_session_id FROM claude_sessions WHERE session_uuid = $1")
            .bind(claude_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pty_link, Some(pty.id));

    let (current,): (Option<Uuid>,) =
        sqlx::query_as("SELECT current_session_uuid FROM pty_sessions WHERE id = $1")
            .bind(pty.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, Some(claude_uuid));

    mgr.delete(pty.id).await.ok();
}

#[tokio::test]
async fn second_claude_session_in_same_pty_updates_pointer() {
    let pool = fresh_pool().await;
    let mgr = common::devenv_backed_pty_manager(pool.clone()).await;
    let pty = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    correlate::apply(
        &pool,
        &CorrelateMsg {
            pty_id: pty.id,
            session_uuid: first,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();
    correlate::apply(
        &pool,
        &CorrelateMsg {
            pty_id: pty.id,
            session_uuid: second,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();

    let (current,): (Option<Uuid>,) =
        sqlx::query_as("SELECT current_session_uuid FROM pty_sessions WHERE id = $1")
            .bind(pty.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, Some(second));

    // Both claude_sessions rows exist; the second is currently pointed at.
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM claude_sessions WHERE session_uuid IN ($1, $2)",
    )
    .bind(first)
    .bind(second)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 2);

    mgr.delete(pty.id).await.ok();
}

#[tokio::test]
async fn resuming_a_session_in_a_new_pty_releases_the_old_pty() {
    let pool = fresh_pool().await;
    let mgr = common::devenv_backed_pty_manager(pool.clone()).await;
    let old_pty = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    let new_pty = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    let session = Uuid::new_v4();
    correlate::apply(
        &pool,
        &CorrelateMsg {
            pty_id: old_pty.id,
            session_uuid: session,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();
    correlate::apply(
        &pool,
        &CorrelateMsg {
            pty_id: new_pty.id,
            session_uuid: session,
            agent: "claude-code".to_string(),
        },
    )
    .await
    .unwrap();

    // The session moved: exactly one PTY row may claim it, and it is the new one.
    let rows: Vec<(Uuid, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT id, current_session_uuid, current_session_agent \
         FROM pty_sessions WHERE id IN ($1, $2)",
    )
    .bind(old_pty.id)
    .bind(new_pty.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    for (id, current, agent) in rows {
        if id == new_pty.id {
            assert_eq!(current, Some(session));
            assert_eq!(agent.as_deref(), Some("claude-code"));
        } else {
            assert_eq!(current, None, "old PTY must release the resumed session");
            assert_eq!(agent, None);
        }
    }

    mgr.delete(old_pty.id).await.ok();
    mgr.delete(new_pty.id).await.ok();
}

#[tokio::test]
async fn socket_listener_accepts_json_line_and_updates_db() {
    let pool = fresh_pool().await;
    let mgr = common::devenv_backed_pty_manager(pool.clone()).await;
    let pty = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    let sock = tmp_sock();
    let sock_for_listener = sock.clone();
    let listener_pool = pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = correlate::run(listener_pool, sock_for_listener).await;
    });

    wait_for_socket(&sock).await;

    let claude_uuid = Uuid::new_v4();
    let mut s = UnixStream::connect(&sock).await.expect("connect");
    let payload = format!(
        "{{\"pty_id\":\"{}\",\"claude_session_uuid\":\"{}\"}}\n",
        pty.id, claude_uuid
    );
    s.write_all(payload.as_bytes()).await.expect("write");

    // Read the ack so we know the server has committed.
    let mut reader = BufReader::new(s);
    let mut ack = String::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut ack)).await;
    assert!(ack.contains("ok"), "expected 'ok' ack, got {ack:?}");

    let (current,): (Option<Uuid>,) =
        sqlx::query_as("SELECT current_session_uuid FROM pty_sessions WHERE id = $1")
            .bind(pty.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, Some(claude_uuid));

    listener_task.abort();
    mgr.delete(pty.id).await.ok();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn runtime_running_waits_for_pty_row_insert() {
    let pool = fresh_pool().await;
    let pty_id = Uuid::new_v4();
    let runtime_pool = pool.clone();
    let runtime_task = tokio::spawn(async move {
        correlate::apply_runtime(
            &runtime_pool,
            &RuntimeMsg {
                pty_id,
                agent: "codex".to_string(),
                event: RuntimeEvent::Running,
                exit_code: None,
            },
        )
        .await
        .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at, \
             agent_runtime_agent, agent_runtime_state, agent_runtime_started_at) \
         VALUES ($1, $2, $3, 'live', NOW(), 'codex', 'starting', NOW())",
    )
    .bind(pty_id)
    .bind("r")
    .bind("/tmp")
    .execute(&pool)
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(2), runtime_task)
        .await
        .expect("runtime update timed out")
        .unwrap();

    let (agent, state): (Option<String>, String) = sqlx::query_as(
        "SELECT agent_runtime_agent, agent_runtime_state \
           FROM pty_sessions WHERE id = $1",
    )
    .bind(pty_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(agent.as_deref(), Some("codex"));
    assert_eq!(state, "running");
}

#[tokio::test(flavor = "current_thread")]
async fn codex_launcher_correlates_session_uuid_from_open_rollout_file() {
    let pool = fresh_pool().await;
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, $2, $3, 'live', NOW())",
    )
    .bind(pty_id)
    .bind("r")
    .bind("/tmp")
    .execute(&pool)
    .await
    .unwrap();

    let sock = tmp_sock();
    let sock_for_listener = sock.clone();
    let listener_pool = pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = correlate::run(listener_pool, sock_for_listener).await;
    });

    wait_for_socket(&sock).await;

    let tmp = tempfile::tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let day_dir = sessions_dir.join("2026").join("04").join("19");
    std::fs::create_dir_all(&day_dir).unwrap();

    let session_uuid = Uuid::new_v4();
    let rollout_path = day_dir.join(format!("rollout-2026-04-19T01-53-43-{session_uuid}.jsonl"));
    assert_eq!(
        sulion::ingest::parse_codex_session_uuid(&rollout_path),
        Some(session_uuid)
    );

    let fake_codex = tmp.path().join("fake-codex.sh");
    write_fake_codex(&fake_codex);

    let code = tokio::time::timeout(
        Duration::from_secs(3),
        run_launcher(LauncherConfig {
            codex_bin: fake_codex,
            pty_id,
            sessions_dir: sessions_dir.clone(),
            correlate_sock: sock.clone(),
            args: vec![rollout_path.into_os_string()],
        }),
    )
    .await
    .expect("launcher timed out")
    .unwrap();
    assert_eq!(code, 0);

    let (current_uuid, current_agent): (Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT current_session_uuid, current_session_agent \
           FROM pty_sessions WHERE id = $1",
    )
    .bind(pty_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current_uuid, Some(session_uuid));
    assert_eq!(current_agent.as_deref(), Some("codex"));

    let (linked_pty, stored_agent): (Option<Uuid>, String) =
        sqlx::query_as("SELECT pty_session_id, agent FROM claude_sessions WHERE session_uuid = $1")
            .bind(session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked_pty, Some(pty_id));
    assert_eq!(stored_agent, "codex");

    listener_task.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test(flavor = "current_thread")]
async fn codex_launcher_ignores_nested_child_rollout_files() {
    let pool = fresh_pool().await;
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, $2, $3, 'live', NOW())",
    )
    .bind(pty_id)
    .bind("r")
    .bind("/tmp")
    .execute(&pool)
    .await
    .unwrap();

    let sock = tmp_sock();
    let sock_for_listener = sock.clone();
    let listener_pool = pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = correlate::run(listener_pool, sock_for_listener).await;
    });

    wait_for_socket(&sock).await;

    let tmp = tempfile::tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let day_dir = sessions_dir.join("2026").join("04").join("19");
    std::fs::create_dir_all(&day_dir).unwrap();

    let root_session_uuid = Uuid::new_v4();
    let child_session_uuid = Uuid::new_v4();
    let root_rollout = day_dir.join(format!(
        "rollout-2026-04-19T01-53-43-{root_session_uuid}.jsonl"
    ));
    let child_rollout = day_dir.join(format!(
        "rollout-2026-04-19T01-53-44-{child_session_uuid}.jsonl"
    ));

    let fake_codex = tmp.path().join("fake-codex-nested.sh");
    write_fake_codex_with_nested_child(&fake_codex);

    let code = tokio::time::timeout(
        Duration::from_secs(3),
        run_launcher(LauncherConfig {
            codex_bin: fake_codex,
            pty_id,
            sessions_dir,
            correlate_sock: sock.clone(),
            args: vec![
                child_rollout.into_os_string(),
                root_rollout.into_os_string(),
            ],
        }),
    )
    .await
    .expect("launcher timed out")
    .unwrap();
    assert_eq!(code, 0);

    let (current_uuid, current_agent): (Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT current_session_uuid, current_session_agent \
           FROM pty_sessions WHERE id = $1",
    )
    .bind(pty_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(current_uuid, Some(root_session_uuid));
    assert_eq!(current_agent.as_deref(), Some("codex"));

    let child_link: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT pty_session_id FROM claude_sessions WHERE session_uuid = $1")
            .bind(child_session_uuid)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(child_link, None);

    listener_task.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test(flavor = "current_thread")]
async fn codex_launcher_exits_when_correlation_ack_never_arrives() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions_dir = tmp.path().join("sessions");
    let day_dir = sessions_dir.join("2026").join("04").join("19");
    std::fs::create_dir_all(&day_dir).unwrap();

    let session_uuid = Uuid::new_v4();
    let rollout_path = day_dir.join(format!("rollout-2026-04-19T01-53-43-{session_uuid}.jsonl"));
    let fake_codex = tmp.path().join("fake-codex.sh");
    write_fake_codex(&fake_codex);

    let sock = tmp_sock();
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = std::io::BufReader::new(stream);
        let mut payload = String::new();
        reader.read_line(&mut payload).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        payload
    });

    let started = tokio::time::Instant::now();
    let code = tokio::time::timeout(
        Duration::from_secs(3),
        run_launcher(LauncherConfig {
            codex_bin: fake_codex,
            pty_id: Uuid::new_v4(),
            sessions_dir,
            correlate_sock: sock.clone(),
            args: vec![rollout_path.into_os_string()],
        }),
    )
    .await
    .expect("launcher timed out")
    .unwrap();
    assert_eq!(code, 0);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "launcher should bound correlation ACK waits"
    );

    let payload = server.join().unwrap();
    assert!(payload.contains(&session_uuid.to_string()));
    assert!(payload.contains("\"agent\":\"codex\""));

    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn control_socket_publishes_plans_and_preserves_explicit_attention() {
    let pool = fresh_pool().await;
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, 'r', '/tmp', 'live', NOW())",
    )
    .bind(pty_id)
    .execute(&pool)
    .await
    .unwrap();

    let sock = tmp_sock();
    let socket_path = sock.clone();
    let listener_pool = pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = correlate::run(listener_pool, socket_path).await;
    });
    wait_for_socket(&sock).await;

    let started = correlate::send_control(
        &sock,
        pty_id,
        ControlRequest::PlanStart {
            title: "Published work".to_string(),
            summary: "Short durable progress".to_string(),
            phases: vec![
                NewPhase {
                    title: "Build".to_string(),
                    description: "Implement it".to_string(),
                    status: None,
                    size: Some("m".to_string()),
                },
                NewPhase {
                    title: "Verify".to_string(),
                    description: "Test it".to_string(),
                    status: None,
                    size: None,
                },
            ],
            all_pending: false,
        },
    )
    .await
    .unwrap();
    assert!(started.ok, "{:?}", started.error);
    let plan = started.data.unwrap();
    assert_eq!(plan["title"], "Published work");
    assert_eq!(plan["phases"][0]["size"], "m");
    assert_eq!(plan["phases"][1]["size"], serde_json::Value::Null);
    assert_eq!(plan["attachments"][0]["pty_session_id"], pty_id.to_string());
    let plan_id = plan["id"].as_str().unwrap().parse::<Uuid>().unwrap();

    let updated = correlate::send_control(
        &sock,
        pty_id,
        ControlRequest::PlanUpdatePhase {
            plan_id: None,
            phase_reference: "1".to_string(),
            input: UpdatePhaseInput {
                title: None,
                description: None,
                status: Some("completed".to_string()),
                status_note: Some("done".to_string()),
                position: None,
                size: None,
            },
        },
    )
    .await
    .unwrap();
    assert!(updated.ok, "{:?}", updated.error);
    assert_eq!(updated.data.unwrap()["phases"][0]["status"], "completed");

    let attention = correlate::send_control(
        &sock,
        pty_id,
        ControlRequest::ActivitySet {
            state: ActivityState::NeedsInput,
            summary: Some("Choose an API shape".to_string()),
            reason: Some("Two durable options remain".to_string()),
        },
    )
    .await
    .unwrap();
    assert!(attention.ok, "{:?}", attention.error);
    assert_eq!(attention.data.unwrap()["state"], "needs_input");

    sulion::activity::set(
        &pool,
        pty_id,
        ActivityState::AwaitingPrompt,
        Some("automatic turn complete"),
        None,
        "ingester",
        "explicit",
    )
    .await
    .unwrap();
    let current = correlate::send_control(&sock, pty_id, ControlRequest::ActivityGet)
        .await
        .unwrap();
    assert!(current.ok, "{:?}", current.error);
    assert_eq!(current.data.unwrap()["state"], "needs_input");

    let history = correlate::send_control(
        &sock,
        pty_id,
        ControlRequest::PlanHistory {
            plan_id: Some(plan_id),
        },
    )
    .await
    .unwrap();
    assert!(history.ok, "{:?}", history.error);
    assert!(history.data.unwrap().as_array().unwrap().len() >= 4);

    listener_task.abort();
    let _ = std::fs::remove_file(&sock);
}

#[tokio::test]
async fn control_socket_sets_and_clears_the_agent_terminal_name() {
    let pool = fresh_pool().await;
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, 'r', '/tmp', 'live', NOW())",
    )
    .bind(pty_id)
    .execute(&pool)
    .await
    .unwrap();

    let sock = tmp_sock();
    let socket_path = sock.clone();
    let listener_pool = pool.clone();
    let listener_task = tokio::spawn(async move {
        let _ = correlate::run(listener_pool, socket_path).await;
    });
    wait_for_socket(&sock).await;

    // Set trims whitespace and echoes the stored value.
    let set = correlate::send_control(
        &sock,
        pty_id,
        ControlRequest::SessionNameSet {
            name: Some("  ingest batcher refactor  ".to_string()),
        },
    )
    .await
    .unwrap();
    assert!(set.ok, "{:?}", set.error);
    assert_eq!(set.data.unwrap()["agent_label"], "ingest batcher refactor");

    let stored: Option<String> =
        sqlx::query_scalar("SELECT agent_label FROM pty_sessions WHERE id = $1")
            .bind(pty_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("ingest batcher refactor"));

    let shown = correlate::send_control(&sock, pty_id, ControlRequest::SessionNameGet)
        .await
        .unwrap();
    assert_eq!(
        shown.data.unwrap()["agent_label"],
        "ingest batcher refactor"
    );

    // Over-long names are refused, not truncated.
    let too_long = correlate::send_control(
        &sock,
        pty_id,
        ControlRequest::SessionNameSet {
            name: Some("x".repeat(101)),
        },
    )
    .await
    .unwrap();
    assert!(!too_long.ok);

    // Clear removes it.
    let cleared =
        correlate::send_control(&sock, pty_id, ControlRequest::SessionNameSet { name: None })
            .await
            .unwrap();
    assert!(cleared.ok);
    let stored: Option<String> =
        sqlx::query_scalar("SELECT agent_label FROM pty_sessions WHERE id = $1")
            .bind(pty_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, None);

    listener_task.abort();
    let _ = std::fs::remove_file(&sock);
}
