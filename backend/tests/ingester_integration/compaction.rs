use super::*;
use serde_json::{json, Value};
use sulion::activity::{self, ActivityState};
use sulion::ingest::{load_timeline_response, rebuild_session_projection, ProjectionFilters};

const SUMMARY: &str = "This session is being continued from a previous conversation.";

fn append(fx: &Fixture, value: Value) {
    fx.append(&format!("{value}\n"));
}

fn initial_turn(fx: &Fixture) {
    append(
        fx,
        json!({"type":"user","uuid":"prompt","timestamp":"2026-10-24T16:00:00Z",
        "message":{"content":"work"}}),
    );
    append(
        fx,
        json!({"type":"assistant","uuid":"answer","timestamp":"2026-10-24T16:01:00Z",
        "message":{"id":"msg","model":"claude-sonnet-4-6","content":[{"type":"text","text":"done"}],
        "usage":{"input_tokens":100,"output_tokens":40}}}),
    );
    append(
        fx,
        json!({"type":"system","uuid":"stop","subtype":"turn_duration",
        "timestamp":"2026-10-24T16:01:01Z"}),
    );
}

fn compaction_records() -> Vec<Value> {
    vec![
        json!({"type":"last-prompt","timestamp":"2026-10-24T16:55:00Z"}),
        json!({"type":"system","uuid":"boundary","subtype":"compact_boundary",
            "timestamp":"2026-10-24T16:55:01Z","content":"Conversation compacted",
            "isMeta":false,"compactMetadata":{"trigger":"auto"}}),
        json!({"type":"user","uuid":"summary","parentUuid":"boundary",
            "timestamp":"2026-10-24T16:55:00.999Z","isCompactSummary":true,
            "isVisibleInTranscriptOnly":true,"turnOrigin":"human","message":{"content":SUMMARY}}),
        json!({"type":"attachment","timestamp":"2026-10-24T16:55:02Z",
            "attachment":{"type":"file"},"rendered":[{"content":"hydrated file contents"}]}),
        json!({"type":"system","subtype":"informational","timestamp":"2026-10-24T16:55:03Z",
            "content":"Compacted while idle, before the prompt cache expired"}),
        json!({"type":"cost-state","totalCostUSD":1.25}),
    ]
}

async fn associate(pool: &db::Pool, fx: &Fixture) -> Uuid {
    let pty = Uuid::new_v4();
    sqlx::query("INSERT INTO pty_sessions (id,repo,working_dir,state,current_session_uuid,agent_runtime_state) \
        VALUES ($1,'test','/repo','live',$2,'running')")
        .bind(pty).bind(fx.session_uuid).execute(pool).await.unwrap();
    sqlx::query("UPDATE claude_sessions SET pty_session_id=$2 WHERE session_uuid=$1")
        .bind(fx.session_uuid)
        .bind(pty)
        .execute(pool)
        .await
        .unwrap();
    pty
}

async fn snapshot(pool: &db::Pool, fx: &Fixture) -> Value {
    serde_json::to_value(
        load_timeline_response(
            pool,
            fx.session_uuid,
            &ProjectionFilters {
                show_bookkeeping: true,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .turns,
    )
    .unwrap()
}

async fn activity_record(
    pool: &db::Pool,
    pty: Uuid,
) -> Result<Option<activity::ActivityRecord>, sqlx::Error> {
    sqlx::query_as(
        "SELECT pty_session_id,state,summary,reason,source,confidence,updated_at \
        FROM session_activity_state WHERE pty_session_id=$1",
    )
    .bind(pty)
    .fetch_optional(pool)
    .await
}

#[tokio::test]
async fn idle_compaction_keeps_activity_timing_and_next_prompt_correct() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    initial_turn(&fx);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let pty = associate(&pool, &fx).await;
    activity::set(
        &pool,
        pty,
        ActivityState::AwaitingPrompt,
        None,
        None,
        "hook",
        "explicit",
    )
    .await
    .unwrap();
    let idle = activity_record(&pool, pty).await.unwrap();
    for record in compaction_records() {
        append(&fx, record);
        Ingester::new().tick(&pool, &fx.config()).await.unwrap();
        let current = activity_record(&pool, pty).await.unwrap();
        assert_eq!(
            serde_json::to_value(current).unwrap(),
            serde_json::to_value(&idle).unwrap()
        );
    }
    let shown = snapshot(&pool, &fx).await;
    assert_eq!(shown.as_array().unwrap().len(), 1);
    assert_eq!(shown[0]["end_timestamp"], "2026-10-24T16:01:01Z");
    assert_eq!(shown[0]["duration_ms"], 61_000);
    assert_eq!(shown[0]["output_tokens"], 40);
    assert!(shown[0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["kind"] == "system"
            && item["subtype"] == "compact_summary"
            && item["text"] == SUMMARY));
    let hidden = load_timeline_response(&pool, fx.session_uuid, &Default::default())
        .await
        .unwrap();
    assert_eq!(
        hidden.turns[0].items.len(),
        1,
        "maintenance hides with bookkeeping"
    );
    let raw: Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE session_uuid=$1 AND event_uuid='summary'",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(raw["type"], "user");
    assert_eq!(raw["isCompactSummary"], true);
    rebuild_session_projection(&pool, fx.session_uuid)
        .await
        .unwrap();
    assert_eq!(snapshot(&pool, &fx).await, shown);
    append(
        &fx,
        json!({"type":"user","uuid":"next","timestamp":"2026-10-24T18:00:00Z",
        "message":{"content":"next task"}}),
    );
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let next = snapshot(&pool, &fx).await;
    assert_eq!(next.as_array().unwrap().len(), 2);
    assert_eq!(next[0]["end_timestamp"], shown[0]["end_timestamp"]);
    assert_eq!(next[1]["user_prompt_text"], "next task");
    assert_eq!(
        activity_record(&pool, pty).await.unwrap().unwrap().state,
        "working"
    );
}

#[tokio::test]
async fn active_compaction_keeps_operations_and_revised_usage_on_the_original_turn() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    append(
        &fx,
        json!({"type":"user","uuid":"prompt","timestamp":"2026-10-24T16:00:00Z",
        "message":{"content":"work"}}),
    );
    append(
        &fx,
        json!({"type":"assistant","timestamp":"2026-10-24T16:01:00Z",
        "message":{"id":"msg","model":"claude-sonnet-4-6",
        "content":[{"type":"tool_use","id":"call","name":"Bash","input":{"command":"true"}}],
        "usage":{"input_tokens":100,"output_tokens":40}}}),
    );
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let original = snapshot(&pool, &fx).await;
    let pty = associate(&pool, &fx).await;
    activity::set(
        &pool,
        pty,
        ActivityState::Working,
        Some("work"),
        None,
        "hook",
        "explicit",
    )
    .await
    .unwrap();
    for record in compaction_records().into_iter().take(4) {
        append(&fx, record);
        Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    }
    assert_eq!(
        activity_record(&pool, pty)
            .await
            .unwrap()
            .unwrap()
            .summary
            .as_deref(),
        Some("work")
    );
    append(
        &fx,
        json!({"type":"user","timestamp":"2026-10-24T16:56:00Z",
        "message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"ok"}]}}),
    );
    append(
        &fx,
        json!({"type":"assistant","timestamp":"2026-10-24T16:56:01Z",
        "message":{"id":"msg","model":"claude-sonnet-4-6","content":[{"type":"text","text":"finished"}],
        "usage":{"input_tokens":100,"output_tokens":50}}}),
    );
    append(
        &fx,
        json!({"type":"assistant","timestamp":"2026-10-24T16:56:02Z",
        "message":{"id":"msg-next","model":"claude-sonnet-4-6","content":[{"type":"text","text":"next response"}],
        "usage":{"input_tokens":200,"output_tokens":7}}}),
    );
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let continued = snapshot(&pool, &fx).await;
    assert_eq!(continued.as_array().unwrap().len(), 1);
    assert_eq!(continued[0]["id"], original[0]["id"]);
    assert_eq!(continued[0]["input_tokens"], 300);
    assert_eq!(continued[0]["output_tokens"], 57);
    assert_eq!(continued[0]["tool_pairs"].as_array().unwrap().len(), 1);
    assert_eq!(continued[0]["tool_pairs"][0]["is_pending"], false);
    assert!(!continued[0]["markdown"].as_str().unwrap().contains(SUMMARY));
    rebuild_session_projection(&pool, fx.session_uuid)
        .await
        .unwrap();
    assert_eq!(snapshot(&pool, &fx).await, continued);
    sqlx::query(
        "UPDATE events SET speaker='user',is_meta=false,subtype=NULL \
        WHERE session_uuid=$1 AND event_uuid='summary'",
    )
    .bind(fx.session_uuid)
    .execute(&pool)
    .await
    .unwrap();
    rebuild_session_projection(&pool, fx.session_uuid)
        .await
        .unwrap();
    let legacy = snapshot(&pool, &fx).await;
    assert_eq!(legacy.as_array().unwrap().len(), 2);
    assert_eq!(legacy[1]["output_tokens"], 7);
    sulion::ingest::mark_projection_versions_current(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ingest_projection_versions SET version=CASE name \
        WHEN 'canonical_blocks' THEN 6 WHEN 'timeline_projection' THEN 15 ELSE version END",
    )
    .execute(&pool)
    .await
    .unwrap();
    sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(snapshot(&pool, &fx).await, continued);
}

#[tokio::test]
async fn upgrade_repairs_compaction_from_database_without_overwriting_explicit_activity() {
    let pool = fresh_pool().await;
    let idle = Fixture::new();
    let explicit = Fixture::new();
    let purged = Fixture::new();
    let unrelated = Fixture::new();
    for fx in [&idle, &explicit, &purged, &unrelated] {
        initial_turn(fx);
        if fx.session_uuid != unrelated.session_uuid {
            for record in compaction_records() {
                append(fx, record);
            }
        }
        Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    }
    let idle_pty = associate(&pool, &idle).await;
    let explicit_pty = associate(&pool, &explicit).await;
    for pty in [idle_pty, explicit_pty] {
        activity::set(
            &pool,
            pty,
            ActivityState::Working,
            Some(SUMMARY),
            None,
            "ingester",
            "derived",
        )
        .await
        .unwrap();
    }
    activity::set(
        &pool,
        explicit_pty,
        ActivityState::Blocked,
        Some("user report"),
        Some("keep this"),
        "agent",
        "explicit",
    )
    .await
    .unwrap();
    let explicit_before = activity_record(&pool, explicit_pty).await.unwrap();
    // Recreate the deployed misclassification, then build its phantom turns.
    for fx in [&idle, &explicit, &purged] {
        sqlx::query("UPDATE events SET speaker=CASE WHEN kind='user' THEN 'user' ELSE 'system' END, \
            is_meta=false, subtype=payload->>'subtype' WHERE session_uuid=$1 AND \
            (payload->>'isCompactSummary'='true' OR subtype IN ('compact_boundary','compact_status'))")
            .bind(fx.session_uuid).execute(&pool).await.unwrap();
        rebuild_session_projection(&pool, fx.session_uuid)
            .await
            .unwrap();
        assert_eq!(snapshot(&pool, fx).await.as_array().unwrap().len(), 2);
    }
    sqlx::query("UPDATE claude_sessions SET purged_at=NOW() WHERE session_uuid=$1")
        .bind(purged.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    let untouched: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT session_uuid,generation FROM timeline_session_state \
        WHERE session_uuid IN ($1,$2) ORDER BY session_uuid",
    )
    .bind(unrelated.session_uuid)
    .bind(purged.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    let digest = snapshot(&pool, &purged).await;
    sulion::ingest::mark_projection_versions_current(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ingest_projection_versions SET version=CASE name \
        WHEN 'canonical_blocks' THEN 6 WHEN 'timeline_projection' THEN 15 ELSE version END",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Startup repair must use stored payloads even after harness retention removes JSONL.
    for fx in [&idle, &explicit, &purged, &unrelated] {
        std::fs::remove_file(fx.jsonl_path()).unwrap();
    }
    let stats = sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(stats.canonical_events_backfilled, 6);
    assert_eq!(stats.timeline_sessions_backfilled, 2);
    for fx in [&idle, &explicit] {
        let repaired = snapshot(&pool, fx).await;
        assert_eq!(repaired.as_array().unwrap().len(), 1);
        assert_eq!(repaired[0]["end_timestamp"], "2026-10-24T16:01:01Z");
        assert_eq!(repaired[0]["output_tokens"], 40);
        let canonical: (String, bool, String) = sqlx::query_as(
            "SELECT speaker,is_meta,subtype \
            FROM events WHERE session_uuid=$1 AND event_uuid='summary'",
        )
        .bind(fx.session_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(canonical, ("system".into(), true, "compact_summary".into()));
    }
    assert_eq!(
        activity_record(&pool, idle_pty)
            .await
            .unwrap()
            .unwrap()
            .state,
        "awaiting_prompt"
    );
    assert_eq!(
        serde_json::to_value(activity_record(&pool, explicit_pty).await.unwrap()).unwrap(),
        serde_json::to_value(explicit_before).unwrap()
    );
    let after: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT session_uuid,generation FROM timeline_session_state \
        WHERE session_uuid IN ($1,$2) ORDER BY session_uuid",
    )
    .bind(unrelated.session_uuid)
    .bind(purged.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(after, untouched);
    assert_eq!(snapshot(&pool, &purged).await, digest);
    let repeated = sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(repeated.canonical_events_backfilled, 0);
    assert_eq!(repeated.timeline_sessions_backfilled, 0);
}
