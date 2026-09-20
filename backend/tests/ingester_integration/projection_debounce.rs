//! The live-turn projection debounce: while a session keeps growing it is
//! projected at most once per interval, and it is projected on the next
//! tick once it stops growing.

use std::time::Duration;

use super::*;

fn claude_line(fx: &Fixture, ts: &str, role: &str, text: &str) {
    fx.append(&format!(
        r#"{{"type":"{role}","timestamp":"{ts}","uuid":"{}","message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#,
        Uuid::new_v4()
    ));
    fx.append("\n");
}

async fn projected_event_count(pool: &db::Pool, session: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(event_count), 0)::BIGINT FROM timeline_turns WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn growing_session_projects_once_per_interval_and_flushes_when_quiet() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let cfg = fx
        .config()
        .with_projection_debounce(Duration::from_secs(30));
    let ingester = Ingester::new();

    // First sighting projects at once.
    claude_line(&fx, "2026-09-19T10:00:00Z", "user", "start");
    claude_line(&fx, "2026-09-19T10:00:01Z", "assistant", "working");
    ingester.tick(&pool, &cfg).await.unwrap();
    assert_eq!(projected_event_count(&pool, fx.session_uuid).await, 2);

    // Growth inside the interval is ingested but not yet projected.
    claude_line(&fx, "2026-09-19T10:00:02Z", "assistant", "more");
    ingester.tick(&pool, &cfg).await.unwrap();
    assert_eq!(event_count(&pool, fx.session_uuid).await, 3);
    assert_eq!(projected_event_count(&pool, fx.session_uuid).await, 2);

    // Still growing on the next tick: still deferred, offset kept.
    claude_line(&fx, "2026-09-19T10:00:03Z", "assistant", "and more");
    ingester.tick(&pool, &cfg).await.unwrap();
    assert_eq!(projected_event_count(&pool, fx.session_uuid).await, 2);

    // A tick with nothing new flushes the deferred work.
    ingester.tick(&pool, &cfg).await.unwrap();
    assert_eq!(projected_event_count(&pool, fx.session_uuid).await, 4);

    // The default configuration projects on every tick.
    let immediate = fx.config();
    claude_line(&fx, "2026-09-19T10:00:04Z", "assistant", "done");
    Ingester::new().tick(&pool, &immediate).await.unwrap();
    assert_eq!(projected_event_count(&pool, fx.session_uuid).await, 5);
}
