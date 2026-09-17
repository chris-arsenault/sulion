//! Model-switch detection through the ingester: Codex thread settings and
//! Claude fallback blocks become `agent_model_switches` rows, the session's
//! baselines move with them, and acknowledgement changes what counts as a
//! departure.

use serde_json::{json, Value};

use super::*;

fn codex_line(fx: &CodexFixture, timestamp: &str, kind: &str, payload: Value) {
    fx.append(&json!({"type": kind, "timestamp": timestamp, "payload": payload}).to_string());
    fx.append("\n");
}

fn codex_settings(fx: &CodexFixture, timestamp: &str, model: &str, effort: &str) {
    codex_line(
        fx,
        timestamp,
        "event_msg",
        json!({
            "type": "thread_settings_applied",
            "thread_id": fx.session_uuid.to_string(),
            "thread_settings": {
                "model": model, "model_provider_id": "openai", "service_tier": "default",
                "reasoning_effort": effort
            }
        }),
    );
}

fn codex_turn_context(
    fx: &CodexFixture,
    timestamp: &str,
    turn_id: &str,
    model: &str,
    effort: &str,
) {
    codex_line(
        fx,
        timestamp,
        "turn_context",
        json!({"turn_id": turn_id, "model": model, "effort": effort, "cwd": "/repo"}),
    );
}

/// A turn lifecycle record: `task_started`, `task_complete`, `turn_aborted`.
fn codex_task(fx: &CodexFixture, timestamp: &str, subtype: &str, turn_id: &str) {
    codex_line(
        fx,
        timestamp,
        "event_msg",
        json!({"type": subtype, "turn_id": turn_id}),
    );
}

fn codex_token_count(fx: &CodexFixture, timestamp: &str, used_percent: f64) {
    codex_line(
        fx,
        timestamp,
        "event_msg",
        json!({
            "type": "token_count",
            "info": { "total_token_usage": { "input_tokens": 10, "cached_input_tokens": 0,
                "cache_write_input_tokens": 0, "output_tokens": 1, "reasoning_output_tokens": 0,
                "total_tokens": 11 }, "model_context_window": 258400 },
            "rate_limits": {
                "limit_id": "codex", "plan_type": "pro", "rate_limit_reached_type": null,
                "primary": { "used_percent": used_percent, "window_minutes": 10080,
                    "resets_at": 1789865705 }
            }
        }),
    );
}

fn claude_assistant(fx: &Fixture, timestamp: &str, model: &str, content: Value) {
    fx.append(
        &json!({
            "type": "assistant", "timestamp": timestamp, "uuid": Uuid::new_v4().to_string(),
            "effort": "high", "cwd": "/repo", "version": "2.1.224",
            "message": { "model": model, "role": "assistant", "content": content,
                "usage": { "service_tier": "standard", "input_tokens": 1, "output_tokens": 1 } }
        })
        .to_string(),
    );
    fx.append("\n");
}

async fn switches(pool: &db::Pool, session: Uuid) -> Vec<Value> {
    sqlx::query_scalar::<_, Value>(
        "SELECT COALESCE(jsonb_agg(to_jsonb(s) - 'id' - 'detected_at' ORDER BY byte_offset), '[]'::jsonb) \
           FROM agent_model_switches s WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_one(pool)
    .await
    .unwrap()
    .as_array()
    .cloned()
    .unwrap_or_default()
}

async fn baselines(pool: &db::Pool, session: Uuid) -> (Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT observed_model, confirmed_model FROM agent_session_metadata WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn codex_thread_settings_switch_is_enforced_and_a_restore_is_not() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    let ingester = Ingester::new();

    // Launch model, one completed turn with a rate-limit snapshot.
    codex_task(&fx, "2026-09-17T07:12:41Z", "task_started", "t1");
    codex_turn_context(&fx, "2026-09-17T07:12:41Z", "t1", "gpt-6-astra", "high");
    codex_token_count(&fx, "2026-09-17T07:46:00Z", 92.0);
    codex_task(&fx, "2026-09-17T07:46:00Z", "task_complete", "t1");
    ingester.tick(&pool, &fx.config()).await.unwrap();
    assert!(switches(&pool, fx.session_uuid).await.is_empty());
    assert_eq!(
        baselines(&pool, fx.session_uuid).await,
        (Some("gpt-6-astra".into()), Some("gpt-6-astra".into()))
    );

    // The harness applies new settings three times in a burst while idle,
    // then the next turn starts on the new model.
    codex_settings(&fx, "2026-09-17T07:50:36.076Z", "gpt-5.6-luna", "medium");
    codex_settings(&fx, "2026-09-17T07:50:36.084Z", "gpt-5.6-luna", "high");
    codex_settings(&fx, "2026-09-17T07:50:36.091Z", "gpt-5.6-luna", "medium");
    codex_task(&fx, "2026-09-17T08:10:13Z", "task_started", "t2");
    codex_turn_context(&fx, "2026-09-17T08:10:13Z", "t2", "gpt-5.6-luna", "medium");
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows.len(), 1, "one switch for the burst: {rows:?}");
    let row = &rows[0];
    assert_eq!(row["source"], "codex_thread_settings");
    assert_eq!(row["from_model"], "gpt-6-astra");
    assert_eq!(row["to_model"], "gpt-5.6-luna");
    assert_eq!(row["from_effort"], "high");
    assert_eq!(row["to_effort"], "medium");
    assert_eq!(row["enforced"], true);
    assert_eq!(row["turn_in_flight"], false);
    assert_eq!(
        row["context"]["rate_limits"]["primary"]["used_percent"],
        92.0
    );
    assert_eq!(row["context"]["rate_limits"]["plan_type"], "pro");
    assert_eq!(row["context"]["service_tier"], "default");
    assert_eq!(
        baselines(&pool, fx.session_uuid).await,
        (Some("gpt-5.6-luna".into()), Some("gpt-6-astra".into()))
    );

    // Restoring the launch model by hand is recorded, not enforced.
    codex_task(&fx, "2026-09-17T08:11:39Z", "task_complete", "t2");
    codex_settings(&fx, "2026-09-17T08:36:08.074Z", "gpt-6-astra", "medium");
    codex_settings(&fx, "2026-09-17T08:36:08.082Z", "gpt-6-astra", "xhigh");
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["from_model"], "gpt-5.6-luna");
    assert_eq!(rows[1]["to_model"], "gpt-6-astra");
    assert_eq!(rows[1]["enforced"], false);

    // A second downgrade after the restore is a fresh departure.
    codex_settings(&fx, "2026-09-17T14:38:23Z", "gpt-5.6-luna", "medium");
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["to_model"], "gpt-5.6-luna");
    assert_eq!(rows[2]["enforced"], true);
}

#[tokio::test]
async fn codex_switch_during_a_turn_names_the_turn() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    codex_task(&fx, "2026-09-17T07:12:41Z", "task_started", "t1");
    codex_turn_context(&fx, "2026-09-17T07:12:41Z", "t1", "gpt-6-astra", "high");
    codex_settings(&fx, "2026-09-17T07:13:00Z", "gpt-5.6-luna", "medium");
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["turn_in_flight"], true);
    assert_eq!(rows[0]["turn_id"], "t1");
}

#[tokio::test]
async fn claude_fallback_block_is_an_enforced_mid_turn_switch() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let ingester = Ingester::new();
    claude_assistant(
        &fx,
        "2026-09-02T00:56:52Z",
        "claude-fable-5",
        json!([{ "type": "text", "text": "working" }]),
    );
    // An API-error record carries a placeholder model, never a baseline.
    claude_assistant(
        &fx,
        "2026-09-02T00:57:00Z",
        "<synthetic>",
        json!([{ "type": "text", "text": "API Error" }]),
    );
    ingester.tick(&pool, &fx.config()).await.unwrap();
    assert!(switches(&pool, fx.session_uuid).await.is_empty());

    claude_assistant(
        &fx,
        "2026-09-02T01:00:00Z",
        "claude-opus-4-8",
        json!([{ "type": "fallback", "from": { "model": "claude-fable-5" }, "to": { "model": "claude-opus-4-8" } }]),
    );
    claude_assistant(
        &fx,
        "2026-09-02T01:00:11Z",
        "claude-opus-4-8",
        json!([{ "type": "text", "text": "continuing on the fallback" }]),
    );
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows.len(), 1, "one switch per model change: {rows:?}");
    assert_eq!(rows[0]["source"], "claude_fallback");
    assert_eq!(rows[0]["from_model"], "claude-fable-5");
    assert_eq!(rows[0]["to_model"], "claude-opus-4-8");
    assert_eq!(rows[0]["enforced"], true);
    assert_eq!(rows[0]["turn_in_flight"], true);
    assert_eq!(rows[0]["context"]["fallback"]["from"], "claude-fable-5");
    assert_eq!(rows[0]["observed_at"], "2026-09-02T01:00:00+00:00");
    let (_, confirmed) = baselines(&pool, fx.session_uuid).await;
    assert_eq!(confirmed.as_deref(), Some("claude-fable-5"));
}

#[tokio::test]
async fn acknowledging_with_adopt_moves_the_confirmed_model() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    let ingester = Ingester::new();
    codex_turn_context(&fx, "2026-09-17T07:12:41Z", "t1", "gpt-6-astra", "high");
    codex_settings(&fx, "2026-09-17T07:50:36Z", "gpt-5.6-luna", "medium");
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let switch_id: Uuid =
        sqlx::query_scalar("SELECT id FROM agent_model_switches WHERE session_uuid = $1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();

    // The switch belongs to a PTY only through the PTY's current session.
    let pty_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state, current_session_uuid) \
         VALUES ($1, 'demo', '/repo', 'live', $2)",
    )
    .bind(pty_id)
    .bind(fx.session_uuid)
    .execute(&pool)
    .await
    .unwrap();

    let other_pty = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id, repo, working_dir, state) VALUES ($1, 'demo', '/repo', 'live')",
    )
    .bind(other_pty)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        !sulion::model_switches::acknowledge(&pool, other_pty, switch_id, true)
            .await
            .unwrap()
    );

    assert!(
        sulion::model_switches::acknowledge(&pool, pty_id, switch_id, true)
            .await
            .unwrap()
    );
    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows[0]["adopted"], true);
    assert!(rows[0]["acknowledged_at"].is_string());
    assert_eq!(
        baselines(&pool, fx.session_uuid).await,
        (Some("gpt-5.6-luna".into()), Some("gpt-5.6-luna".into()))
    );
    // Already closed: a second acknowledgement finds nothing open.
    assert!(
        !sulion::model_switches::acknowledge(&pool, pty_id, switch_id, false)
            .await
            .unwrap()
    );

    // With the new model confirmed, going back to the launch model is the
    // departure now.
    codex_settings(&fx, "2026-09-17T09:00:00Z", "gpt-6-astra", "high");
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let rows = switches(&pool, fx.session_uuid).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["to_model"], "gpt-6-astra");
    assert_eq!(rows[1]["enforced"], true);
    let listed = sulion::model_switches::list_for_pty(&pool, pty_id)
        .await
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].to_model, "gpt-6-astra");
}
