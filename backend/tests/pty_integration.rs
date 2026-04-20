//! PTY lifecycle integration tests. Require a test Postgres — gated behind
//! `SULION_TEST_DB`. The supported path is `make test-rust-integration`.

use std::path::PathBuf;
use std::time::Duration;

use sulion::db;
use sulion::pty::{
    default_shell, read_meta, reconcile_orphans_on_startup, PtyManager, PtyState, SpawnParams,
};

fn test_db_url() -> Option<String> {
    std::env::var("SULION_TEST_DB").ok()
}

async fn fresh_pool() -> db::Pool {
    let url = test_db_url().expect("SULION_TEST_DB");
    let pool = db::connect(&url).await.expect("connect");
    // Clean slate between tests — they run serially under cargo's default
    // test harness when sharing a DB, but --test-threads=1 is what the
    // runner uses below.
    sqlx::query(
        "TRUNCATE events, ingester_state, claude_sessions, pty_sessions, repos RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .ok();
    db::run_migrations(&pool).await.expect("migrate");
    pool
}

async fn wait_for_state(
    pool: &db::Pool,
    id: uuid::Uuid,
    want: PtyState,
    timeout: Duration,
) -> PtyState {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut observed = PtyState::Live;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(meta)) = read_meta(pool, id).await {
            observed = meta.state;
            if observed == want {
                return observed;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    observed
}

#[tokio::test]
#[ignore]
async fn spawn_persists_row_in_live_state() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());
    let meta = mgr
        .spawn(SpawnParams {
            repo: "testrepo".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: default_shell(),
            ..Default::default()
        })
        .await
        .expect("spawn");

    assert_eq!(meta.state, PtyState::Live);
    let from_db = read_meta(&pool, meta.id).await.unwrap().unwrap();
    assert_eq!(from_db.state, PtyState::Live);
    assert_eq!(from_db.repo, "testrepo");

    // Clean up.
    mgr.delete(meta.id).await.expect("delete");
}

#[tokio::test]
#[ignore]
async fn child_exit_transitions_to_dead() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());
    // `/bin/true` exits immediately with code 0.
    let meta = mgr
        .spawn(SpawnParams {
            repo: "test".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/true"),
            ..Default::default()
        })
        .await
        .expect("spawn");

    let observed = wait_for_state(&pool, meta.id, PtyState::Dead, Duration::from_secs(5)).await;
    assert_eq!(observed, PtyState::Dead);
    let from_db = read_meta(&pool, meta.id).await.unwrap().unwrap();
    assert_eq!(from_db.exit_code, Some(0));
    assert!(from_db.ended_at.is_some());
}

#[tokio::test]
#[ignore]
async fn delete_terminates_live_session_and_marks_deleted() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());
    // `/bin/sleep 60` stays live long enough for us to kill it.
    let meta = mgr
        .spawn(SpawnParams {
            repo: "test".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .expect("spawn");

    mgr.delete(meta.id).await.expect("delete");

    let from_db = read_meta(&pool, meta.id).await.unwrap().unwrap();
    assert_eq!(from_db.state, PtyState::Deleted);
    assert!(from_db.ended_at.is_some());
}

#[tokio::test]
#[ignore]
async fn list_returns_live_and_dead_but_not_deleted() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());

    // Live
    let live = mgr
        .spawn(SpawnParams {
            repo: "r1".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .unwrap();

    // Dead (exits immediately)
    let dead = mgr
        .spawn(SpawnParams {
            repo: "r2".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/true"),
            ..Default::default()
        })
        .await
        .unwrap();
    wait_for_state(&pool, dead.id, PtyState::Dead, Duration::from_secs(5)).await;

    // Deleted
    let deleted = mgr
        .spawn(SpawnParams {
            repo: "r3".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sleep"),
            args: vec!["60".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    mgr.delete(deleted.id).await.unwrap();

    let listed = mgr.list().await.unwrap();
    let ids: Vec<uuid::Uuid> = listed.iter().map(|m| m.id).collect();
    assert!(ids.contains(&live.id), "live should appear");
    assert!(ids.contains(&dead.id), "dead should appear");
    assert!(!ids.contains(&deleted.id), "deleted must not appear");

    mgr.delete(live.id).await.unwrap();
}

#[tokio::test]
#[ignore]
async fn startup_reconciliation_transitions_live_rows_to_orphaned() {
    let pool = fresh_pool().await;
    // Insert a fake 'live' row simulating a PTY from the prior backend.
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) \
         VALUES ($1, $2, $3, 'live', NOW() - INTERVAL '1 hour')",
    )
    .bind(id)
    .bind("prior")
    .bind("/tmp")
    .execute(&pool)
    .await
    .unwrap();

    let reconciled = reconcile_orphans_on_startup(&pool).await.unwrap();
    assert_eq!(reconciled, 1);

    let meta = read_meta(&pool, id).await.unwrap().unwrap();
    assert_eq!(meta.state, PtyState::Orphaned);
    assert!(meta.ended_at.is_some());

    // Running it again with no live rows is a no-op.
    let reconciled = reconcile_orphans_on_startup(&pool).await.unwrap();
    assert_eq!(reconciled, 0);
}

#[tokio::test]
#[ignore]
async fn pty_id_env_is_propagated_to_shell() {
    let pool = fresh_pool().await;
    let mgr = PtyManager::new(pool.clone());

    // The shell will write SULION_PTY_ID to a tempfile, exit, and
    // we'll read the file to verify the env var was propagated. Using a
    // file avoids racing the broadcast-channel subscriber against the
    // shell's exit.
    let tmp = std::env::temp_dir().join(format!("sulion-envcheck-{}.txt", uuid::Uuid::new_v4()));
    let tmp_str = tmp.to_string_lossy().into_owned();
    let cmd = format!("printf '%s' \"$SULION_PTY_ID\" > {tmp_str}; exit 0");

    let meta = mgr
        .spawn(SpawnParams {
            repo: "r".into(),
            working_dir: PathBuf::from("/tmp"),
            shell: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), cmd],
            ..Default::default()
        })
        .await
        .unwrap();

    wait_for_state(&pool, meta.id, PtyState::Dead, Duration::from_secs(5)).await;

    let contents = std::fs::read_to_string(&tmp).expect("read envcheck file");
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(
        contents.trim(),
        meta.id.to_string(),
        "shell should have seen SULION_PTY_ID={}",
        meta.id
    );
}
