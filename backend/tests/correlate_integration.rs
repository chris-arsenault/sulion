#![cfg(feature = "integration-tests")]

//! Correlation socket integration tests. Exercise the SessionStart-hook
//! path end-to-end: bind a socket, write a JSON line, verify the DB rows.

use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use sulion::codex::{run_launcher, LauncherConfig};
use sulion::correlate::{self, CorrelateMsg};
use sulion::db;
use sulion::pty::{PtyManager, SpawnParams};
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
        "TRUNCATE events, ingester_state, claude_sessions, pty_sessions, repos RESTART IDENTITY CASCADE",
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

#[tokio::test]
async fn apply_upserts_claude_session_and_points_pty() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());

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
    let mgr = PtyManager::new(pool.clone());
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
async fn socket_listener_accepts_json_line_and_updates_db() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());
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
