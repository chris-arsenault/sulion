use super::*;
use serde_json::{json, Value};

fn claude(fx: &Fixture, value: Value) {
    fx.append(&format!("{value}\n"));
}

fn receipt(fx: &Fixture, id: &str, time: &str, model: &str, output: i64) {
    claude(
        fx,
        json!({"type":"assistant", "uuid":Uuid::new_v4(), "timestamp":time,
        "message":{"id":id,"model":model,"role":"assistant","content":[{"type":"text","text":"answer"}],
            "usage":{"input_tokens":100,"cache_read_input_tokens":200,"output_tokens":output}}}),
    );
}

async fn usage(pool: &db::Pool, session: Uuid) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object( \
        'session',(SELECT to_jsonb(s)-'updated_at' FROM agent_session_usage s WHERE session_uuid=$1), \
        'daily',(SELECT jsonb_agg(to_jsonb(d)-'updated_at' ORDER BY day) FROM agent_usage_daily d WHERE session_uuid=$1), \
        'models',(SELECT jsonb_agg(to_jsonb(m)-'updated_at'-'last_usage_message_id' ORDER BY day,model) FROM agent_model_usage_daily m WHERE session_uuid=$1))")
        .bind(session).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn claude_revisions_survive_restart_interleaving_and_projection_repair() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    claude(
        &fx,
        json!({"type":"user","uuid":"prompt","timestamp":"2026-09-22T23:58:00Z","message":{"content":"work"}}),
    );
    receipt(&fx, "a", "2026-09-22T23:59:00Z", "model-a", 5);
    receipt(&fx, "b", "2026-09-22T23:59:01Z", "model-a", 20);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    receipt(&fx, "a", "2026-09-23T00:00:01Z", "model-b", 268);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    receipt(&fx, "a", "2026-09-23T00:00:02Z", "model-b", 250);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let before = usage(&pool, fx.session_uuid).await;
    assert_eq!(before["session"]["input_tokens"], 200);
    assert_eq!(before["session"]["output_tokens"], 270);
    assert_eq!(before["models"][0]["output_tokens"], 20);
    assert_eq!(before["models"][1]["output_tokens"], 250);
    let tokens: i64 = sqlx::query_scalar(
        "SELECT SUM(output_tokens)::BIGINT FROM timeline_turns WHERE session_uuid=$1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tokens, 270);
    rebuild_ingest_derivatives(&pool).await.unwrap();
    assert_eq!(usage(&pool, fx.session_uuid).await, before);
    // Repair also fixes the previously deployed first-receipt totals.
    sqlx::query("UPDATE agent_session_usage SET output_tokens=25 WHERE session_uuid=$1")
        .bind(fx.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE ingest_projection_versions SET version=2 WHERE name='usage_projection'")
        .execute(&pool)
        .await
        .unwrap();
    sulion::ingest::run_required_startup_maintenance(&pool)
        .await
        .unwrap();
    assert_eq!(usage(&pool, fx.session_uuid).await, before);
    receipt(&fx, "b", "2026-09-23T00:00:03Z", "model-b", 30);
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let moved = usage(&pool, fx.session_uuid).await;
    rebuild_ingest_derivatives(&pool).await.unwrap();
    assert_eq!(usage(&pool, fx.session_uuid).await, moved);
}

#[tokio::test]
async fn queued_prompts_and_paste_bodies_are_projected_once_with_raw_evidence_intact() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let pasted =
        "\n\n<pasted_content id=\"8b80\">\n  preserve indentation\n</pasted_content id=\"8b80\">\n";
    claude(
        &fx,
        json!({"type":"attachment","timestamp":"2026-09-23T01:00:00Z","attachment":{
        "type":"queued_command","commandMode":"prompt","origin":{"kind":"human"},"humanTurn":true,
        "source_uuid":"queued-prompt","prompt":pasted}}),
    );
    claude(
        &fx,
        json!({"type":"user","uuid":"queued-prompt","timestamp":"2026-09-23T01:00:01Z","message":{"content":pasted}}),
    );
    claude(
        &fx,
        json!({"type":"attachment","timestamp":"2026-09-23T01:00:02Z","attachment":{
        "type":"queued_command","commandMode":"prompt","origin":{"kind":"task-notification"},"prompt":"not a user"}}),
    );
    // The harness may reuse a paste id in a separate human turn.
    claude(
        &fx,
        json!({"type":"user","uuid":"next-prompt","timestamp":"2026-09-23T01:00:03Z","message":{"content":pasted}}),
    );
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let prompts: Vec<String> = sqlx::query_scalar(
        "SELECT user_prompt_text FROM timeline_turns WHERE session_uuid=$1 ORDER BY turn_ord",
    )
    .bind(fx.session_uuid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        prompts,
        vec!["  preserve indentation", "  preserve indentation"]
    );
    let raw: String = sqlx::query_scalar("SELECT payload #>> '{attachment,prompt}' FROM events WHERE session_uuid=$1 AND byte_offset=0")
        .bind(fx.session_uuid).fetch_one(&pool).await.unwrap();
    assert_eq!(raw, pasted);
    let pty = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO pty_sessions (id,repo,working_dir,state) VALUES ($1,'test','/repo','dead')",
    )
    .bind(pty)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE claude_sessions SET pty_session_id=$2 WHERE session_uuid=$1")
        .bind(fx.session_uuid)
        .bind(pty)
        .execute(&pool)
        .await
        .unwrap();
    let submitted = sulion::submitted_prompts::record(
        &pool,
        pty,
        Some("claude"),
        "\n\t preserve indentation\n",
        false,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE submitted_prompts SET submitted_at='2026-09-23T00:59:59Z' WHERE id=$1")
        .bind(submitted)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sulion::submitted_prompts::reconcile(&pool).await.unwrap(),
        1
    );
    rebuild_ingest_derivatives(&pool).await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM timeline_turns WHERE session_uuid=$1")
            .bind(fx.session_uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn codex_started_activity_links_the_actual_child_to_the_spawn_pair() {
    let pool = fresh_pool().await;
    let parent = CodexFixture::new();
    let child = CodexFixture::new();
    codex(
        &parent,
        0,
        "session_meta",
        json!({"id":parent.session_uuid}),
    );
    codex(&parent, 1, "turn_context", json!({"turn_id":"parent-turn"}));
    codex(
        &parent,
        2,
        "response_item",
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"delegate work"}]}),
    );
    codex(
        &parent,
        3,
        "response_item",
        json!({"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"spawn",
        "arguments":"{\"task_name\":\"worker\",\"message\":\"inspect parser\"}"}),
    );
    codex(
        &parent,
        4,
        "event_msg",
        json!({"type":"item_completed","item":{"type":"SubAgentActivity","kind":"started","id":"spawn",
        "agent_thread_id":child.session_uuid,"agent_path":"/root/worker"}}),
    );
    codex(
        &parent,
        5,
        "response_item",
        json!({"type":"function_call_output","call_id":"spawn","output":"started"}),
    );
    Ingester::new().tick(&pool, &parent.config()).await.unwrap();
    codex(
        &child,
        0,
        "session_meta",
        json!({"id":child.session_uuid,"forked_from_id":parent.session_uuid,"subagent_history_start_ordinal":2}),
    );
    codex(&child, 1, "session_meta", json!({"id":parent.session_uuid}));
    codex(&child, 6, "turn_context", json!({"turn_id":"child-turn"}));
    codex(
        &child,
        7,
        "response_item",
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"child findings"}]}),
    );
    codex(
        &child,
        8,
        "response_item",
        json!({"type":"function_call","name":"exec_command","call_id":"inspect","arguments":"{\"cmd\":\"pwd\"}"}),
    );
    codex(
        &child,
        9,
        "response_item",
        json!({"type":"function_call_output","call_id":"inspect","output":"/repo"}),
    );
    Ingester::new().tick(&pool, &child.config()).await.unwrap();
    let (name, raw, input, subagent): (String,Option<String>,Value,Value) = sqlx::query_as(
        "SELECT name,raw_name,input,subagent_json FROM timeline_operations WHERE session_uuid=$1 AND pair_id='spawn'")
        .bind(parent.session_uuid).fetch_one(&pool).await.unwrap();
    assert_eq!(name, "task");
    assert_eq!(raw.as_deref(), Some("collaboration.spawn_agent"));
    assert_eq!(input["prompt"], "inspect parser");
    assert_eq!(input["description"], "worker");
    assert!(subagent.to_string().contains("child findings"));
    assert!(subagent.to_string().contains("inspect"));
}

fn codex(fx: &CodexFixture, ordinal: u64, kind: &str, payload: Value) {
    fx.append(&format!("{}\n", json!({"type":kind,"ordinal":ordinal,"timestamp":format!("2026-09-23T02:00:{ordinal:02}Z"),"payload":payload})));
}

#[tokio::test]
async fn codex_inherited_history_never_overwrites_child_lineage_after_restart_or_repair() {
    let pool = fresh_pool().await;
    let fx = CodexFixture::new();
    let parent = Uuid::new_v4();
    codex(
        &fx,
        0,
        "session_meta",
        json!({"id":fx.session_uuid,"forked_from_id":parent,"subagent_history_start_ordinal":5,"cli_version":"0.155.1"}),
    );
    codex(
        &fx,
        1,
        "session_meta",
        json!({"id":parent,"cli_version":"0.154.0"}),
    );
    codex(
        &fx,
        2,
        "turn_context",
        json!({"turn_id":"parent-turn","model":"parent-model"}),
    );
    codex(
        &fx,
        3,
        "response_item",
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"inherited prompt"}]}),
    );
    codex(
        &fx,
        4,
        "event_msg",
        json!({"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"output_tokens":100}}}),
    );
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    codex(
        &fx,
        5,
        "turn_context",
        json!({"turn_id":"child-turn","model":"child-model"}),
    );
    codex(
        &fx,
        6,
        "response_item",
        json!({"type":"agent_message","id":"amsg-1","author":"/root","recipient":"/root/worker",
        "content":[{"type":"input_text","text":"inspect the parser"},{"type":"encrypted_content","encrypted_content":"opaque"}]}),
    );
    codex(
        &fx,
        7,
        "response_item",
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"child answer"}]}),
    );
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    for rebuild in [false, true] {
        if rebuild {
            rebuild_ingest_derivatives(&pool).await.unwrap();
        }
        let rows: Vec<(bool, Option<String>, Option<String>)> = sqlx::query_as("SELECT is_sidechain,parent_event_uuid,subtype FROM events WHERE session_uuid=$1 ORDER BY byte_offset")
            .bind(fx.session_uuid).fetch_all(&pool).await.unwrap();
        assert!(rows[1..5]
            .iter()
            .all(|r| r.2.as_deref() == Some("inherited_history")));
        assert_eq!(rows[7].1.as_deref(), Some("child-turn"));
        assert!(rows[7].0);
        let markdown: String = sqlx::query_scalar(
            "SELECT string_agg(markdown,'\n') FROM timeline_turns WHERE session_uuid=$1",
        )
        .bind(fx.session_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!markdown.contains("inherited prompt"));
        assert!(markdown.contains("inspect the parser"));
        assert!(markdown.contains("Encrypted agent message"));
        let usages: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_session_usage WHERE session_uuid=$1")
                .bind(fx.session_uuid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(usages, 0);
    }
}
