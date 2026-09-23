use serde_json::{json, Value};

use super::*;

fn append(fx: &CodexFixture, timestamp: &str, kind: &str, payload: Value) {
    fx.append(&json!({"type": kind, "timestamp": timestamp, "payload": payload}).to_string());
    fx.append("\n");
}

fn tokens(input: i64, cached: i64, output: i64) -> Value {
    json!({"input_tokens": input, "cached_input_tokens": cached,
        "cache_write_input_tokens": 0, "output_tokens": output,
        "reasoning_output_tokens": 0, "total_tokens": input + output})
}

fn count(fx: &CodexFixture, time: &str, input: i64, cached: i64, output: i64) {
    append(
        fx,
        time,
        "event_msg",
        json!({"type": "token_count", "info": {
            "total_token_usage": tokens(input, cached, output),
            "last_token_usage": tokens(20, 10, 5), "model_context_window": 100_000
        }}),
    );
}

fn response(fx: &CodexFixture, time: &str, id: &str, input: i64, cached: i64, output: i64) {
    append(
        fx,
        time,
        "token_usage_record",
        json!({
            "response_id": id, "usage": tokens(input, cached, output)
        }),
    );
}

async fn snapshot(pool: &db::Pool, session: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object( \
            'session', (SELECT to_jsonb(s) - 'updated_at' FROM agent_session_usage s WHERE session_uuid=$1), \
            'daily', (SELECT jsonb_agg(to_jsonb(d) - 'updated_at' ORDER BY day) FROM agent_usage_daily d WHERE session_uuid=$1), \
            'models', (SELECT jsonb_agg(to_jsonb(m) - 'updated_at' - 'last_usage_message_id' ORDER BY day,model) \
                FROM agent_model_usage_daily m WHERE session_uuid=$1), \
            'responses', (SELECT jsonb_agg(response_id ORDER BY response_id) FROM agent_usage_responses WHERE session_uuid=$1))",
    ).bind(session).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn response_usage_includes_compaction_and_preserves_legacy_prefix_across_rebuild() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    let t1 = "2026-09-07T23:59:00Z";
    let t2 = "2026-09-08T00:01:00Z";
    append(&fx, t1, "turn_context", json!({"model":"gpt-5.6-sol"}));
    count(&fx, t1, 100, 60, 10);
    // A resumed legacy session starts emitting the per-response ledger.
    response(&fx, t1, "ordinary", 50, 30, 5);
    count(&fx, t1, 150, 90, 15);
    // Compaction has no matching token_count and must still count as spend.
    response(&fx, t1, "compaction", 200, 180, 40);
    append(&fx, t1, "compacted", json!({}));
    append(&fx, t2, "turn_context", json!({"model":"gpt-6-astra"}));
    response(&fx, t2, "next-model", 80, 50, 8);
    // Context counters can reset without resetting spend.
    count(&fx, t2, 80, 50, 8);
    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let before = snapshot(&pool, fx.session_uuid).await;
    assert_eq!(before["session"]["input_tokens"], 110);
    assert_eq!(before["session"]["cached_input_tokens"], 320);
    assert_eq!(before["session"]["output_tokens"], 63);
    assert_eq!(before["session"]["total_tokens"], 493);
    assert_eq!(before["session"]["context_tokens"], 25);
    assert_eq!(before["session"]["model_context_window"], 100_000);
    assert_eq!(before["session"]["codex_response_records"], true);
    assert_eq!(before["models"][0]["model"], "gpt-5.6-sol");
    assert_eq!(before["models"][0]["input_tokens"], 80);
    assert_eq!(before["models"][0]["output_tokens"], 55);
    assert_eq!(before["models"][1]["model"], "gpt-6-astra");
    assert_eq!(before["models"][1]["output_tokens"], 8);

    // A non-adjacent duplicate at a new offset after restarting adds nothing.
    response(&fx, t2, "ordinary", 50, 30, 5);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    assert_eq!(snapshot(&pool, fx.session_uuid).await, before);

    // Simulate the previous deployed projection version and verify repair,
    // including durable receipt identities and context-only updates.
    let legacy = CodexFixture::new();
    count(&legacy, t1, 200, 120, 20);
    Ingester::new().tick(&pool, &legacy.config()).await.unwrap();
    let legacy_before = snapshot(&pool, legacy.session_uuid).await;
    sqlx::query("UPDATE agent_session_usage SET output_tokens=0, codex_response_records=false WHERE session_uuid=$1")
        .bind(fx.session_uuid).execute(&pool).await.unwrap();
    sqlx::query("UPDATE agent_model_usage_daily SET output_tokens=0 WHERE session_uuid=$1")
        .bind(fx.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ingest_projection_versions (name, version) VALUES ('usage_projection', 1) ON CONFLICT (name) DO UPDATE SET version=1")
        .execute(&pool)
        .await
        .unwrap();
    let stats = sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(stats.usage_sessions_backfilled, 2);
    assert_eq!(snapshot(&pool, fx.session_uuid).await, before);
    assert_eq!(snapshot(&pool, legacy.session_uuid).await, legacy_before);
    let second = sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(second.usage_sessions_backfilled, 0);

    // The rebuilt state must remain correct when live ingestion resumes.
    response(&fx, t2, "compaction", 200, 180, 40);
    response(&fx, t2, "after-repair", 10, 0, 2);
    count(&fx, t2, 90, 50, 10);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let after = snapshot(&pool, fx.session_uuid).await;
    assert_eq!(after["session"]["total_tokens"], 505);
    assert_eq!(after["models"][1]["input_tokens"], 40);
    assert_eq!(after["models"][1]["output_tokens"], 10);
}

#[tokio::test]
async fn response_only_sessions_rebuild_without_inventing_a_model_or_context() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    response(&fx, "2026-09-08T00:01:00Z", "only", 100, 80, 20);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let before = snapshot(&pool, fx.session_uuid).await;
    assert_eq!(before["session"]["total_tokens"], 120);
    assert_eq!(before["session"]["context_tokens"], Value::Null);
    assert_eq!(before["models"][0]["model"], "(unknown model)");
    sqlx::query("DELETE FROM ingest_projection_versions WHERE name='usage_projection'")
        .execute(&pool)
        .await
        .unwrap();
    let stats = sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(stats.usage_sessions_backfilled, 1);
    assert_eq!(snapshot(&pool, fx.session_uuid).await, before);
}

#[tokio::test]
async fn malformed_records_do_not_disable_legacy_usage() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    let time = "2026-09-08T00:01:00Z";
    append(
        &fx,
        time,
        "token_usage_record",
        json!({"usage": tokens(100,80,20)}),
    );
    append(
        &fx,
        time,
        "token_usage_record",
        json!({"response_id":"missing-usage"}),
    );
    append(
        &fx,
        time,
        "token_usage_record",
        json!({"response_id":"empty-usage", "usage":{}}),
    );
    count(&fx, time, 100, 80, 20);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let usage = snapshot(&pool, fx.session_uuid).await;
    assert_eq!(usage["session"]["total_tokens"], 120);
    assert_eq!(usage["session"]["codex_response_records"], false);
    assert_eq!(usage["responses"], Value::Null);
}
