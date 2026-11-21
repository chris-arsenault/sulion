use super::*;
use serde_json::Value;

#[tokio::test]
async fn plans_rank_before_transcripts_and_expose_phase_sources_and_audit() {
    let pool = fresh_pool().await;
    let unrelated = seed_retrieval_fixture(&pool).await;
    sqlx::query("UPDATE event_blocks SET text = 'society magic ebb flow' WHERE session_uuid = $1")
        .bind(unrelated)
        .execute(&pool)
        .await
        .unwrap();
    let session = seed_empty_session(&pool, "sulion").await;
    let (plan, first, second) = seed_plan(&pool, session).await;
    let h = Harness::new(pool.clone()).await;

    let before = search(&h, "society magic ebb flow", "sulion", None).await;
    assert_eq!(before["results"][0]["source_kind"], "assistant_text");

    // Project the conversation after the plan and audit already exist.
    seed_turns(&pool, session).await;
    let found = search(&h, "society magic ebb flow", "sulion", None).await;
    assert_eq!(found["results"][0]["source_kind"], "plan", "{found:#}");
    assert_eq!(found["results"][0]["plan_id"], plan.to_string());
    assert_eq!(found["results"][0]["summary"], "Connect world systems");
    assert_eq!(
        found["results"][0]["agent_session_uuid"],
        session.to_string()
    );
    assert_eq!(found["results"][0]["turn_id"], 7);
    assert_eq!(found["results"][1]["source_kind"], "assistant_text");

    let phase_hit = search(&h, "regional depletion", "sulion", None).await;
    assert_eq!(phase_hit["results"][0]["phase_id"], second.to_string());
    let other_repo = search(&h, "society magic ebb flow", "other", None).await;
    assert_eq!(other_repo["results"], json!([]));
    let other_session = search(&h, "society magic ebb flow", "sulion", Some(unrelated)).await;
    assert_eq!(other_session["results"][0]["source_kind"], "assistant_text");
    assert_eq!(other_session["results"].as_array().unwrap().len(), 1);

    let complete = record(&h, plan, None).await;
    let sources = complete["turns"].as_array().unwrap();
    let linked_ids: std::collections::BTreeSet<_> = sources
        .iter()
        .map(|source| source["turn_id"].as_i64().unwrap())
        .collect();
    assert_eq!(linked_ids, [7, 19, 26, 33, 46, 67].into_iter().collect());
    assert!(sources.iter().any(|source| source["turn_id"] == 7
        && source["phase_id"].is_null()
        && source["kinds"] == json!(["planning_context"])));
    assert!(sources.iter().any(|source| source["turn_id"] == 26
        && source["phase_id"] == first.to_string()
        && source["kinds"] == json!(["execution"])));
    assert!(sources.iter().any(|source| source["turn_id"] == 33
        && source["phase_id"].is_null()
        && source["kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("execution"))));

    let phase = record(&h, plan, Some(second)).await;
    assert_eq!(phase["plan"]["phases"].as_array().unwrap().len(), 1);
    assert_eq!(phase["plan"]["phases"][0]["id"], second.to_string());
    assert_eq!(phase["turns"].as_array().unwrap().len(), 1);
    assert_eq!(phase["turns"][0]["turn_id"], 33);
    assert_eq!(phase["turns"][0]["kinds"], json!(["change"]));
    assert_eq!(phase["turns"][0]["event_ids"][0], phase["history"][1]["id"]);
    assert_eq!(phase["history"][1]["event_type"], "authoring_changed");
    let change = &phase["history"][1]["authoring_change"]["changes"][0];
    assert_eq!(change["before"]["description"], "Local rules");
    assert_eq!(change["after"]["description"], "Regional depletion");

    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_sulion"))
        .args([
            "retrieve",
            "plan",
            &plan.to_string(),
            "--phase",
            &second.to_string(),
            "--history",
            "--json",
        ])
        .env("SULION_RETRIEVAL_URL", &h.base)
        .env("SULION_RETRIEVAL_TOKEN", "test-token")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        phase
    );
}

async fn search(h: &Harness, text: &str, repo: &str, session: Option<Uuid>) -> Value {
    let mut request = h.client.get(format!("{}/v1/search", h.base)).query(&[
        ("q", text),
        ("search_mode", "lexical"),
        ("repo", repo),
    ]);
    if let Some(session) = session {
        request = request.query(&[
            ("scope", "session"),
            ("agent_session_uuid", &session.to_string()),
        ]);
    }
    get(h, request).await
}

async fn record(h: &Harness, plan_id: Uuid, phase_id: Option<Uuid>) -> Value {
    let mut request = h
        .client
        .get(format!("{}/v1/plans", h.base))
        .query(&json!({"plan_id": plan_id, "history": true}));
    if let Some(phase_id) = phase_id {
        request = request.query(&json!({"phase_id": phase_id}));
    }
    get(h, request).await
}

async fn get(h: &Harness, request: reqwest::RequestBuilder) -> Value {
    let response = h.auth(request).send().await.unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body:#}");
    body
}

async fn seed_plan(pool: &db::Pool, session: Uuid) -> (Uuid, Uuid, Uuid) {
    let plan = Uuid::new_v4();
    let old = Uuid::new_v4();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let pty: Uuid =
        sqlx::query_scalar("SELECT pty_session_id FROM claude_sessions WHERE session_uuid = $1")
            .bind(session)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO plans (id, root_plan_id, repo_name, title, summary, created_by_pty_id, \
             created_by_agent_session_uuid, created_at, updated_at) \
         VALUES ($1, $1, 'sulion', 'World systems', 'Connect world systems', $3, $4, \
                 to_timestamp(20), to_timestamp(70)), \
                ($2, $2, 'sulion', 'Earlier plan', '', $3, $4, to_timestamp(0), to_timestamp(5))",
    )
    .bind(plan)
    .bind(old)
    .bind(pty)
    .bind(session)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plan_attachments (id, plan_id, pty_session_id, agent_session_uuid, attached_at, detached_at) \
         VALUES (gen_random_uuid(), $1, $3, $4, to_timestamp(20), to_timestamp(70)), \
                (gen_random_uuid(), $2, $3, $4, to_timestamp(0), to_timestamp(5))",
    ).bind(plan).bind(old).bind(pty).bind(session).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO plan_phases (id, plan_id, position, title, description, status) \
         VALUES ($1, $3, 1, 'Work out sources', '', 'in_progress'), \
                ($2, $3, 2, 'Verify consumption', 'Regional depletion', 'completed')",
    )
    .bind(first)
    .bind(second)
    .bind(plan)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plan_events (plan_id, phase_id, event_type, actor_kind, agent_session_uuid, \
                to_status, created_at, authoring_change) \
         VALUES ($1, NULL, 'plan_created', 'agent', $4, 'active', to_timestamp(20), NULL), \
                ($1, $2, 'phase_added', 'agent', $4, 'in_progress', to_timestamp(20), NULL), \
                ($1, $3, 'phase_added', 'agent', $4, 'in_progress', to_timestamp(30), NULL), \
                ($1, NULL, 'authoring_changed', 'agent', $4, NULL, to_timestamp(34), $5), \
                ($1, $3, 'phase_status_changed', 'agent', $4, 'completed', to_timestamp(40), NULL), \
                ($1, NULL, 'plan_status_changed', 'agent', $4, 'paused', to_timestamp(50), NULL), \
                ($1, NULL, 'plan_status_changed', 'agent', $4, 'active', to_timestamp(60), NULL)",
    ).bind(plan).bind(first).bind(second).bind(session).bind(json!({"changes": [{
        "op": "edit_phase", "id": second,
        "before": {"description": "Local rules"}, "after": {"description": "Regional depletion"}
    }]})).execute(pool).await.unwrap();
    (plan, first, second)
}

async fn seed_turns(pool: &db::Pool, session: Uuid) {
    sqlx::query(
        "INSERT INTO timeline_turns \
            (session_uuid, turn_id, turn_ord, is_sidechain_turn, preview, user_prompt_text, \
             start_timestamp, end_timestamp, duration_ms, event_count, operation_count, thinking_count, markdown) \
         SELECT $1, turn, turn, turn = 27, prompt, prompt, to_timestamp(turn), to_timestamp(turn + 3), \
                3000, 1, 0, 0, '' \
         FROM (VALUES \
            (2, 'Previous plan'), (7, 'How does society magic ebb and flow?'), (19, 'Write the plan'), \
            (26, 'Implement the first phase'), (27, 'Worker sidechain'), (33, 'Revise consumption'), \
            (46, 'Finish the source rules'), (55, 'While paused'), (67, 'Retest sources'), \
            (74, 'After detaching')) AS turns(turn, prompt)",
    ).bind(session).execute(pool).await.unwrap();
}
