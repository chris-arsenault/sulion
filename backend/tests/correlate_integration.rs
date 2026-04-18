//! Correlation socket integration tests. Exercise the SessionStart-hook
//! path end-to-end: bind a socket, write a JSON line, verify the DB rows.

use std::path::PathBuf;
use std::time::Duration;

use shuttlecraft::correlate::{self, CorrelateMsg};
use shuttlecraft::db;
use shuttlecraft::pty::{PtyManager, SpawnParams};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

fn test_db_url() -> Option<String> {
    std::env::var("SHUTTLECRAFT_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SHUTTLECRAFT_TEST_DB");
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
    std::env::temp_dir().join(format!("shuttlecraft-corr-{}.sock", Uuid::new_v4()))
}

#[tokio::test]
#[ignore]
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
            claude_session_uuid: claude_uuid,
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
        sqlx::query_as("SELECT current_claude_session_uuid FROM pty_sessions WHERE id = $1")
            .bind(pty.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, Some(claude_uuid));

    mgr.delete(pty.id).await.ok();
}

#[tokio::test]
#[ignore]
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
            claude_session_uuid: first,
        },
    )
    .await
    .unwrap();
    correlate::apply(
        &pool,
        &CorrelateMsg {
            pty_id: pty.id,
            claude_session_uuid: second,
        },
    )
    .await
    .unwrap();

    let (current,): (Option<Uuid>,) =
        sqlx::query_as("SELECT current_claude_session_uuid FROM pty_sessions WHERE id = $1")
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
#[ignore]
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

    // Wait for the socket file to appear.
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(sock.exists(), "socket never created");

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
        sqlx::query_as("SELECT current_claude_session_uuid FROM pty_sessions WHERE id = $1")
            .bind(pty.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current, Some(claude_uuid));

    listener_task.abort();
    mgr.delete(pty.id).await.ok();
    let _ = std::fs::remove_file(&sock);
}
