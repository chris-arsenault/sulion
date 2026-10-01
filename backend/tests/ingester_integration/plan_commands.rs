use super::*;
use serde_json::{json, Value};
use sulion::ingest::{turn_stream, OperationCategory, ProjectionFilters};

fn claude_calls(fx: &Fixture, calls: &[(&str, &str)]) {
    fx.append(&format!("{}\n", json!({"type":"user","uuid":"prompt","timestamp":"2026-10-01T01:00:00Z","message":{"content":"work"}})));
    let content: Vec<_> = calls.iter().map(|(id, command)| json!({"type":"tool_use","id":id,"name":"Bash","input":{"command":command}})).collect();
    fx.append(&format!("{}\n", json!({"type":"assistant","uuid":"calls","timestamp":"2026-10-01T01:00:01Z","message":{"content":content}})));
}

async fn operations(pool: &db::Pool, session: Uuid) -> Vec<Value> {
    sqlx::query_scalar("SELECT to_jsonb(o) FROM timeline_operations o WHERE session_uuid=$1 ORDER BY turn_id,operation_ord")
        .bind(session).fetch_all(pool).await.unwrap()
}

#[tokio::test]
async fn claude_plans_filter_and_stream_with_metadata_and_error_evidence() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let command = "sulion plan phase set 2 completed --note 'Checks passed'";
    claude_calls(
        &fx,
        &[
            ("plan", command),
            ("mixed", "sulion plan current && git status"),
            ("example", "echo 'sulion plan close --completed'"),
        ],
    );
    fx.append(&format!("{}\n", json!({"type":"user","uuid":"results","timestamp":"2026-10-01T01:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"plan","content":"No current plan","is_error":true}]}})));
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let rows = operations(&pool, fx.session_uuid).await;
    let plan = rows.iter().find(|row| row["pair_id"] == "plan").unwrap();
    assert_eq!(plan["operation_category"], "plan");
    assert_eq!(plan["operation_type"], "sulion_plan");
    assert_eq!(plan["input"]["command"], command);
    assert_eq!(
        plan["input"]["plan_commands"][0],
        json!({"action":"phase set","phase":"2","status":"completed","note":"Checks passed"})
    );
    assert_eq!(plan["result_content"], "No current plan");
    assert_eq!(plan["is_error"], true);
    let mixed = rows.iter().find(|row| row["pair_id"] == "mixed").unwrap();
    assert_eq!(mixed["operation_category"], "utility");
    assert_eq!(mixed["input"]["plan_commands"][0]["action"], "current");
    assert!(
        rows.iter().find(|row| row["pair_id"] == "example").unwrap()["input"]
            .get("plan_commands")
            .is_none()
    );
    let filters = ProjectionFilters {
        hidden_operation_categories: [OperationCategory::Plan].into(),
        ..Default::default()
    };
    let turn_id = plan["turn_id"].as_i64().unwrap();
    let detail =
        sulion::ingest::load_timeline_turn_detail(&pool, fx.session_uuid, turn_id, &filters)
            .await
            .unwrap()
            .unwrap();
    let items = serde_json::to_string(&detail.items).unwrap();
    assert!(!items.contains("\"pair_id\":\"plan\""));
    assert!(items.contains("\"pair_id\":\"mixed\""));
    let summary = sulion::ingest::load_timeline_summary_response(
        &pool,
        fx.session_uuid,
        &ProjectionFilters::default(),
    )
    .await
    .unwrap();
    assert!(serde_json::to_string(&summary)
        .unwrap()
        .contains("sulion_plan"));
    let header = turn_stream::begin(&pool, fx.session_uuid, turn_id, None)
        .await
        .unwrap();
    let turn_stream::Record::Header { mut cursor, .. } = header else {
        panic!("header")
    };
    let mut streamed = Vec::new();
    loop {
        match turn_stream::next(&pool, fx.session_uuid, turn_id, cursor)
            .await
            .unwrap()
        {
            turn_stream::Record::Batch {
                operations,
                cursor: next,
                ..
            } => {
                streamed.extend(operations);
                cursor = next;
            }
            turn_stream::Record::Complete { .. } => break,
            _ => panic!("unexpected stream record"),
        }
    }
    let compact = streamed.iter().find(|row| row["id"] == "plan").unwrap();
    assert_eq!(compact["category"], "plan");
    assert_eq!(compact["input"]["plan_commands"][0]["phase"], "2");
    assert!(compact["input"]["plan_commands"][0].get("note").is_none());
    let body = turn_stream::operation_body(&pool, fx.session_uuid, turn_id, "plan")
        .await
        .unwrap();
    assert_eq!(body["input"]["plan_commands"][0]["note"], "Checks passed");
    assert_eq!(body["result"]["content"], "No current plan");
}

#[tokio::test]
async fn codex_code_mode_plans_preserve_mixed_edit_classification() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    for (id, code) in [
        ("pure", "text(await tools.exec_command({cmd: \"sulion plan start 'Visible plans' --phase 'Verify|Tests|m'\"}));"),
        ("mixed", r#"text(await tools.exec_command({cmd: 'sulion plan current'})); text(await tools.apply_patch("*** Begin Patch\n*** Add File: /repo/new.txt\n+content\n*** End Patch"));"#),
    ] {
        fx.append(&format!("{}\n", json!({"type":"response_item","timestamp":"2026-10-01T02:00:00Z","payload":{"type":"custom_tool_call","name":"exec","call_id":id,"input":code}})));
    }
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let rows = operations(&pool, fx.session_uuid).await;
    let pure = rows.iter().find(|row| row["pair_id"] == "pure").unwrap();
    assert_eq!(pure["operation_category"], "plan");
    assert_eq!(pure["input"]["plan_commands"][0]["title"], "Visible plans");
    assert_eq!(pure["input"]["plan_commands"][0]["phase_count"], 1);
    let mixed = rows.iter().find(|row| row["pair_id"] == "mixed").unwrap();
    assert_eq!(mixed["operation_category"], "create_content");
    assert_eq!(mixed["input"]["plan_commands"][0]["action"], "current");
    assert_eq!(mixed["input"]["file_edits"][0]["path"], "/repo/new.txt");
    assert_eq!(mixed["input"]["plan_commands"].as_array().unwrap().len(), 1);
}

async fn snapshot(pool: &db::Pool, session: Uuid) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object( \
        'turns',(SELECT jsonb_agg(to_jsonb(t) ORDER BY turn_id) FROM timeline_turns t WHERE session_uuid=$1), \
        'items',(SELECT jsonb_agg(to_jsonb(i) ORDER BY turn_id,byte_offset) FROM timeline_items i WHERE session_uuid=$1), \
        'state',(SELECT to_jsonb(s) FROM timeline_session_state s WHERE session_uuid=$1))")
        .bind(session).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn upgrade_enriches_history_in_place_once_and_skips_purged_sessions() {
    let pool = fresh_pool().await;
    let active = Fixture::new();
    let purged = Fixture::new();
    let unrelated = Fixture::new();
    for fx in [&active, &purged] {
        claude_calls(
            fx,
            &[("plan", "sulion plan current"), ("other", "git status")],
        );
        Ingester::new().tick(&pool, &fx.config()).await.unwrap();
        sqlx::query("UPDATE timeline_operations SET operation_type='bash',operation_category='utility',input=input-'plan_commands' WHERE session_uuid=$1 AND pair_id='plan'")
            .bind(fx.session_uuid).execute(&pool).await.unwrap();
    }
    claude_calls(&unrelated, &[("other", "git status")]);
    Ingester::new()
        .tick(&pool, &unrelated.config())
        .await
        .unwrap();
    sqlx::query("UPDATE claude_sessions SET purged_at=NOW() WHERE session_uuid=$1")
        .bind(purged.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    sulion::ingest::mark_projection_versions_current(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE ingest_projection_versions SET version=13 WHERE name='timeline_projection'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let before = snapshot(&pool, active.session_uuid).await;
    let old_rows = operations(&pool, active.session_uuid).await;
    let purged_before = snapshot(&pool, purged.session_uuid).await;
    let purged_rows = operations(&pool, purged.session_uuid).await;
    let unrelated_before = snapshot(&pool, unrelated.session_uuid).await;
    let turn = old_rows[0]["turn_id"].as_i64().unwrap();
    let turn_stream::Record::Header { cursor, .. } =
        turn_stream::begin(&pool, active.session_uuid, turn, None)
            .await
            .unwrap()
    else {
        panic!("header")
    };
    let stats = sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(stats.timeline_sessions_backfilled, 1);
    let after = snapshot(&pool, active.session_uuid).await;
    assert_eq!(after["turns"], before["turns"]);
    assert_eq!(after["items"], before["items"]);
    assert_eq!(
        after["state"]["projected_through"],
        before["state"]["projected_through"]
    );
    assert_eq!(
        after["state"]["revision"].as_i64().unwrap(),
        before["state"]["revision"].as_i64().unwrap() + 1
    );
    assert_ne!(after["state"]["generation"], before["state"]["generation"]);
    assert!(matches!(
        turn_stream::next(&pool, active.session_uuid, turn, cursor)
            .await
            .unwrap(),
        turn_stream::Record::Reset
    ));
    let rows = operations(&pool, active.session_uuid).await;
    assert_eq!(rows[1], old_rows[1]);
    assert_eq!(rows[0]["turn_id"], old_rows[0]["turn_id"]);
    assert_eq!(rows[0]["operation_ord"], old_rows[0]["operation_ord"]);
    assert_eq!(rows[0]["changed_at"], old_rows[0]["changed_at"]);
    assert_eq!(rows[0]["operation_category"], "plan");
    assert_eq!(snapshot(&pool, purged.session_uuid).await, purged_before);
    assert_eq!(operations(&pool, purged.session_uuid).await, purged_rows);
    assert_eq!(
        snapshot(&pool, unrelated.session_uuid).await,
        unrelated_before
    );
    sqlx::query(
        "UPDATE ingest_projection_versions SET version=13 WHERE name='timeline_projection'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        sulion::ingest::run_required_startup_maintenance(&pool)
            .await
            .unwrap()
            .timeline_sessions_backfilled,
        0
    );
    assert_eq!(snapshot(&pool, active.session_uuid).await, after);
}
