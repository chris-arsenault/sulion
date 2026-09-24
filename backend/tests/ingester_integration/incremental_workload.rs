//! Work the incremental writer does as history grows. Each test prints what
//! it measured (run with `--nocapture`) and asserts the shape the contract
//! requires, with bounds loose enough for a shared test machine.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sulion::ingest::rebuild_session_projection;

use super::*;

fn line(fx: &Fixture, value: serde_json::Value) {
    fx.append(&value.to_string());
    fx.append("\n");
}

fn call_and_result(fx: &Fixture, id: &str) {
    line(
        fx,
        serde_json::json!({"type":"assistant","timestamp":"2026-09-24T10:00:01Z","message":{"role":"assistant",
            "content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":"cargo check"}}]}}),
    );
    line(
        fx,
        serde_json::json!({"type":"user","timestamp":"2026-09-24T10:00:02Z","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":id,"content":"ok"}]}}),
    );
}

fn prompt(fx: &Fixture, text: &str) {
    line(
        fx,
        serde_json::json!({"type":"user","timestamp":"2026-09-24T10:00:00Z","uuid":Uuid::new_v4(),
            "message":{"role":"user","content":text}}),
    );
}

async fn row_versions(pool: &db::Pool, session: Uuid) -> std::collections::HashMap<String, String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT 'op:' || turn_id || ':' || operation_ord, xmin::TEXT FROM timeline_operations WHERE session_uuid = $1 \
         UNION ALL \
         SELECT 'item:' || byte_offset, xmin::TEXT FROM timeline_items WHERE session_uuid = $1 \
         UNION ALL \
         SELECT 'turn:' || turn_id, xmin::TEXT FROM timeline_turns WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter().collect()
}

/// One call and its result appended to a turn of `calls` earlier calls:
/// (existing rows rewritten, rows added, median milliseconds).
async fn append_cost(calls: usize) -> (usize, usize, f64) {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    prompt(&fx, "long turn");
    for n in 0..calls {
        call_and_result(&fx, &format!("c{n}"));
    }
    let ingester = Ingester::new();
    while event_count(&pool, fx.session_uuid).await < (1 + 2 * calls) as i64 {
        ingester.tick(&pool, &fx.config()).await.unwrap();
    }

    let mut timings = Vec::new();
    let (mut rewritten, mut added) = (0, 0);
    for sample in 0..5 {
        let before = row_versions(&pool, fx.session_uuid).await;
        call_and_result(&fx, &format!("tail{sample}"));
        let started = Instant::now();
        ingester.tick(&pool, &fx.config()).await.unwrap();
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
        let after = row_versions(&pool, fx.session_uuid).await;
        rewritten = before
            .iter()
            .filter(|(key, version)| after.get(*key) != Some(*version))
            .count();
        added = after.len() - before.len();
    }
    timings.sort_by(f64::total_cmp);
    (rewritten, added, timings[timings.len() / 2])
}

#[tokio::test]
async fn an_append_does_not_grow_with_the_turn_before_it() {
    let (small_rewritten, small_added, small_ms) = append_cost(200).await;
    let (large_rewritten, large_added, large_ms) = append_cost(2000).await;
    println!(
        "append after 200 calls: {small_rewritten} rows rewritten, {small_added} added, {small_ms:.1} ms; \
         after 2000 calls: {large_rewritten} rewritten, {large_added} added, {large_ms:.1} ms"
    );
    // The turn row is the one existing row an append rewrites.
    assert_eq!((small_rewritten, small_added), (1, 2));
    assert_eq!((large_rewritten, large_added), (1, 2));
    assert!(
        large_ms < small_ms * 3.0 + 20.0,
        "append took {large_ms:.1} ms after 2000 calls vs {small_ms:.1} ms after 200"
    );
}

async fn rebuild_time(calls: usize) -> f64 {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    for turn in 0..calls / 50 {
        prompt(&fx, &format!("turn {turn}"));
        for n in 0..50 {
            call_and_result(&fx, &format!("c{turn}-{n}"));
        }
    }
    let ingester = Ingester::new();
    let lines = (calls / 50) * 101;
    while event_count(&pool, fx.session_uuid).await < lines as i64 {
        ingester.tick(&pool, &fx.config()).await.unwrap();
    }
    let started = Instant::now();
    rebuild_session_projection(&pool, fx.session_uuid)
        .await
        .unwrap();
    started.elapsed().as_secs_f64() * 1000.0
}

#[tokio::test]
async fn replaying_twice_the_input_takes_about_twice_the_work() {
    let single = rebuild_time(1000).await;
    let double = rebuild_time(2000).await;
    println!(
        "rebuild of 2020 events: {single:.0} ms; of 4040 events: {double:.0} ms; ratio {:.2}",
        double / single
    );
    assert!(double / single < 3.0, "ratio {:.2}", double / single);
}

/// A quiet session's prompt shows while a busy transcript's backlog drains.
/// Source-to-ingest is the file write to its event row; ingest-to-visible is
/// the event row to its timeline turn.
#[tokio::test]
async fn a_quiet_session_shows_while_a_busy_backlog_drains() {
    let pool = fresh_pool().await;
    let busy = Fixture::new();
    prompt(&busy, "busy");
    for n in 0..5000 {
        call_and_result(&busy, &format!("b{n}"));
    }
    let ingester = Arc::new(Ingester::new());
    let running = {
        let ingester = ingester.clone();
        let pool = pool.clone();
        let cfg = busy.config();
        tokio::spawn(async move { ingester.run(pool, cfg).await })
    };
    // Let the backlog start draining before the quiet session writes.
    let busy_started = Instant::now();
    while event_count(&pool, busy.session_uuid).await == 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let quiet_session = Uuid::new_v4();
    let quiet_path = busy
        .root
        .path()
        .join(&busy.project_hash)
        .join(format!("{quiet_session}.jsonl"));
    let written = Instant::now();
    std::fs::write(
        &quiet_path,
        format!(
            "{}\n",
            serde_json::json!({"type":"user","timestamp":"2026-09-24T10:00:00Z","uuid":"quiet",
                "message":{"role":"user","content":"quiet prompt"}})
        ),
    )
    .unwrap();
    let deadline = written + Duration::from_secs(60);
    while event_count(&pool, quiet_session).await == 0 {
        assert!(Instant::now() < deadline, "quiet event never ingested");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let ingested = Instant::now();
    loop {
        let turns: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM timeline_turns WHERE session_uuid = $1")
                .bind(quiet_session)
                .fetch_one(&pool)
                .await
                .unwrap();
        if turns > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "quiet turn never projected");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let visible = Instant::now();
    let busy_ingested = event_count(&pool, busy.session_uuid).await;
    let busy_projected: i64 = sqlx::query_scalar(
        "SELECT projected_through FROM timeline_session_state WHERE session_uuid = $1",
    )
    .bind(busy.session_uuid)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .unwrap_or(-1);
    running.abort();
    println!(
        "quiet session: source-to-ingest {} ms, ingest-to-visible {} ms; busy session had \
         {busy_ingested} of 10001 events ingested, projected through offset {busy_projected}, \
         {} ms after it started",
        (ingested - written).as_millis(),
        (visible - ingested).as_millis(),
        (visible - busy_started).as_millis(),
    );
    assert!(
        busy_ingested < 10_001,
        "the backlog finished before the quiet session was measured; raise its size"
    );
    assert!((visible - written) < Duration::from_secs(15));
}
