use super::*;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::Value;

#[tokio::test]
async fn plan_connector_authenticates_and_shares_plans_across_repositories() {
    let h = Harness::new().await;
    let issuer_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", issuer_listener.local_addr().unwrap());
    let jwks =
        serde_json::from_str::<Value>(include_str!("../fixtures/auth-test-jwks.json")).unwrap();
    let authority = axum::Router::new()
        .route(
            "/.well-known/jwks.json",
            axum::routing::get(move || async move { axum::Json(jwks) }),
        )
        .route(
            "/v1/auth/check",
            axum::routing::post(|| async { reqwest::StatusCode::NO_CONTENT }),
        );
    let issuer_task =
        tokio::spawn(async { axum::serve(issuer_listener, authority).await.unwrap() });
    let broker_url = std::env::var_os("SULION_SECRET_BROKER_URL");
    let broker_token = std::env::var_os("SULION_SECRET_BROKER_REGISTRATION_TOKEN");
    std::env::set_var("SULION_SECRET_BROKER_URL", &issuer);
    std::env::set_var("SULION_SECRET_BROKER_REGISTRATION_TOKEN", "fixture");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let auth = sulion::auth::AuthState::new(sulion::config::AuthConfig {
        issuer_url: issuer.clone(),
        client_id: "fixture-browser".into(),
    });
    let app = sulion::api::test_plan_mcp_router(h.state.clone(), Arc::new(auth));
    let server = tokio::spawn(async { axum::serve(listener, app).await.unwrap() });
    let url = format!("{base}/mcp");
    let metadata: Value = h
        .client
        .get(format!("{base}/.well-known/oauth-protected-resource"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(metadata["resource"], "https://sulion.test/mcp");
    assert_eq!(
        metadata["authorization_servers"],
        json!(["https://sulion.test"])
    );
    let oauth: Value = h
        .client
        .get(format!("{base}/.well-known/oauth-authorization-server"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(oauth["issuer"], issuer);
    assert_eq!(
        oauth["authorization_endpoint"],
        "https://login.sulion.test/oauth2/authorize"
    );
    assert_eq!(
        oauth["token_endpoint"],
        "https://login.sulion.test/oauth2/token"
    );
    assert_eq!(oauth["code_challenge_methods_supported"], json!(["S256"]));
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture".into());
    let key =
        EncodingKey::from_rsa_pem(include_bytes!("../fixtures/auth-test-private.pem")).unwrap();
    let sign = |subject, username, client, audience: Option<&str>| {
        let now = chrono::Utc::now().timestamp();
        let mut claims = json!({"sub": subject, "username": username, "client_id": client, "token_use": "access", "iss": issuer, "iat": now, "auth_time": now, "exp": now + 300});
        if let Some(audience) = audience {
            claims["aud"] = json!(audience);
        }
        encode(&header, &claims, &key).unwrap()
    };
    let token = sign(
        "fixture-user",
        "fixture-owner",
        "fixture-mcp",
        Some("https://sulion.test/mcp"),
    );
    let unbound = sign("fixture-user", "fixture-owner", "fixture-mcp", None);
    let request = || {
        h.client
            .post(&url)
            .header("Host", "sulion.test")
            .header("Accept", "application/json, text/event-stream")
    };
    let challenge = request().send().await.unwrap();
    assert_eq!(challenge.status(), reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenge.headers()["www-authenticate"],
        "Bearer resource_metadata=\"https://sulion.test/.well-known/oauth-protected-resource\""
    );
    let wrong_token = sign(
        "fixture-user",
        "fixture-owner",
        "fixture-browser",
        Some("https://sulion.test/mcp"),
    );
    assert_eq!(
        request()
            .bearer_auth(wrong_token)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let wrong_audience = sign(
        "fixture-user",
        "fixture-owner",
        "fixture-mcp",
        Some("https://other.test/mcp"),
    );
    assert_eq!(
        request()
            .bearer_auth(wrong_audience)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    let other_user = sign(
        "other-user",
        "other-account",
        "fixture-mcp",
        Some("https://sulion.test/mcp"),
    );
    assert_eq!(
        request()
            .bearer_auth(other_user)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::FORBIDDEN
    );
    let initialize: Value = request().bearer_auth(unbound).json(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "fixture", "version": "1"}}})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert!(
        initialize["result"]["capabilities"]["tools"].is_object(),
        "{initialize}"
    );
    let rpc = |body: Value| {
        let request = request()
            .bearer_auth(&token)
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&body);
        async move {
            request
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        }
    };
    let listed = rpc(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
    assert!(listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "update_plan_entry"));
    assert!(!listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "save_plan"));
    let denied = rpc(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "list_plans", "arguments": {"repo": "../alpha"}}})).await;
    assert_eq!(denied["result"]["isError"], true);
    sqlx::query(
        "INSERT INTO repo_runtime_state (repo_name, path) VALUES ('alpha', '/repos/alpha')",
    )
    .execute(&h.state.pool)
    .await
    .unwrap();
    let second_repo_plan: Value = h
        .client
        .post(format!("{}/api/repos/second-repo/plans", h.base))
        .json(&json!({"title": "Second project", "phases": [{"title": "Build"}]}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let repositories = rpc(json!({"jsonrpc": "2.0", "id": 12, "method": "tools/call", "params": {"name": "list_plan_repositories", "arguments": {}}})).await;
    assert_eq!(
        repositories["result"]["structuredContent"]["repositories"],
        json!(["alpha", "second-repo"])
    );
    let second_repo_id = &second_repo_plan["id"];
    let plans = rpc(json!({"jsonrpc": "2.0", "id": 14, "method": "tools/call", "params": {"name": "list_plans", "arguments": {"repo": "second-repo"}}})).await;
    assert_eq!(
        plans["result"]["structuredContent"]["plans"][0]["id"],
        *second_repo_id
    );
    let read = rpc(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "read_plan", "arguments": {"plan_id": second_repo_id}}})).await;
    assert_eq!(
        read["result"]["structuredContent"]["plan"]["id"],
        *second_repo_id
    );
    let edited = rpc(json!({"jsonrpc": "2.0", "id": 13, "method": "tools/call", "params": {"name": "update_plan", "arguments": {"plan_id": second_repo_id, "summary": "Edited from MCP"}}})).await;
    assert_eq!(
        edited["result"]["structuredContent"]["plan"]["summary"],
        "Edited from MCP"
    );
    let opened = rpc(json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "create_plan", "arguments": {"repo": "alpha", "title": "City design", "phases": [{"title": "Build"}]}}})).await;
    let plan = &opened["result"]["structuredContent"]["plan"];
    let id = plan["id"].as_str().unwrap_or_else(|| panic!("{opened}"));
    let phase_id = &plan["phases"][0]["id"];
    let edited = rpc(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "add_plan_entry", "arguments": {
        "plan_id": id, "kind": "example", "text": "An inn seeds development",
        "generalization": "Use site conditions and the template to choose the seed"
    }}})).await;
    assert_eq!(edited["result"]["isError"], false, "{edited}");
    let example = &edited["result"]["structuredContent"]["plan"]["authoring"]["entries"][0]["id"];
    assert!(example.is_string());
    let phase_update = rpc(json!({"jsonrpc": "2.0", "id": 10, "method": "tools/call", "params": {"name": "update_plan_phase", "arguments": {
        "plan_id": id, "id": phase_id, "implementation": "Extend the current field producer",
        "acceptance": "The next round consumes the updated fields"
    }}})).await;
    assert_eq!(phase_update["result"]["isError"], false, "{phase_update}");
    let patch = json!({"jsonrpc": "2.0", "id": 11, "method": "tools/call", "params": {"name": "patch_plan", "arguments": {
        "plan_id": id, "revision": 2, "operations": [
            {"op": "edit_plan", "summary": "Refine city fabric"},
            {"op": "edit_entry", "id": example, "rationale": "The site's conditions drive the choice"}
        ]
    }}});
    let batched = rpc(patch.clone()).await;
    assert_eq!(batched["result"]["isError"], false, "{batched}");
    let stale = rpc(patch).await;
    assert_eq!(stale["result"]["isError"], true);
    let review = rpc(json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "read_plan", "arguments": {"plan_id": id, "phase_id": phase_id}}})).await;
    let reviewed = &review["result"]["structuredContent"];
    assert_eq!(reviewed["focus_phase"], *phase_id);
    assert_eq!(reviewed["revision"], 3);
    assert!(reviewed["guidance"]["hypothesis"].is_string(), "{review}");
    assert!(reviewed["plan"].get("attachments").is_none());
    let read: Value = h
        .client
        .get(format!("{}/api/plans/{id}", h.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read["authoring"]["entries"][0]["id"], *example);
    assert_eq!(
        read["authoring"]["entries"][0]["rationale"],
        "The site's conditions drive the choice"
    );
    assert_eq!(read["summary"], "Refine city fabric");
    assert_eq!(read["phases"][0]["status"], "pending");
    assert_eq!(read["attachments"], json!([]));
    verify_terminal_handoff(&h, id, &read).await;
    let call = |name: &str, arguments: Value| {
        rpc(
            json!({"jsonrpc": "2.0", "id": 20, "method": "tools/call", "params": {
                "name": name, "arguments": arguments
            }}),
        )
    };
    let workspace_id = Uuid::new_v4();
    let room_id = "a".repeat(48);
    let registered = call("register_arconis_diagram", json!({
        "repo": "alpha", "workspace_id": workspace_id, "room_id": room_id, "title": "City fabric"
    })).await;
    let diagram = &registered["result"]["structuredContent"]["diagram"];
    assert_eq!(diagram["workspace_id"], json!(workspace_id), "{registered}");
    assert_eq!(
        diagram["url"],
        format!("https://arconis.ahara.io/#room={room_id}")
    );
    let other_repo = call(
        "attach_arconis_diagram",
        json!({
            "plan_id": second_repo_id, "workspace_id": workspace_id, "phase_links": []
        }),
    )
    .await;
    assert_eq!(other_repo["result"]["isError"], true);
    let bad_phase = call("attach_arconis_diagram", json!({
        "plan_id": id, "workspace_id": workspace_id,
        "phase_links": [{"phase_id": second_repo_plan["phases"][0]["id"], "object_id": "streets"}]
    })).await;
    assert_eq!(bad_phase["result"]["isError"], true);
    let linked = call(
        "attach_arconis_diagram",
        json!({
            "plan_id": id, "workspace_id": workspace_id,
            "phase_links": [{"phase_id": phase_id, "object_id": "streets"}]
        }),
    )
    .await;
    let linked_plan = &linked["result"]["structuredContent"]["plan"];
    assert_eq!(
        linked_plan["diagrams"][0]["phase_links"][0]["object_id"], "streets",
        "{linked}"
    );
    assert_eq!(
        linked_plan["phases"][0]["url"],
        format!(
            "https://sulion.test/?plan={id}&phase={}",
            phase_id.as_str().unwrap()
        )
    );
    assert_eq!(
        linked_plan["diagrams"][0]["phase_links"][0]["url"],
        format!("https://arconis.ahara.io/#room={room_id}&item=streets")
    );
    let rest_read: Value = h
        .client
        .get(format!("{}/api/plans/{id}", h.base))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(rest_read["diagrams"], linked_plan["diagrams"]);
    let new_plan = call(
        "create_plan",
        json!({
            "repo": "alpha", "title": "City drainage", "phases": [{"title": "Drain streets"}]
        }),
    )
    .await;
    let new_plan = &new_plan["result"]["structuredContent"]["plan"];
    let discovered = call("list_repo_arconis_diagrams", json!({"repo": "alpha"})).await;
    assert_eq!(
        discovered["result"]["structuredContent"]["diagrams"],
        json!([diagram])
    );
    let reused = call(
        "attach_arconis_diagram",
        json!({
            "plan_id": new_plan["id"], "revision": 0, "workspace_id": workspace_id,
            "phase_links": [{"phase_id": new_plan["phases"][0]["id"], "object_id": "streets"}]
        }),
    )
    .await;
    assert_eq!(reused["result"]["isError"], false, "{reused}");
    let detached = call(
        "detach_arconis_diagram",
        json!({
            "plan_id": new_plan["id"], "revision": 1, "workspace_id": workspace_id
        }),
    )
    .await;
    assert_eq!(detached["result"]["isError"], false, "{detached}");
    let discovered_again = call("list_repo_arconis_diagrams", json!({"repo": "alpha"})).await;
    assert_eq!(
        discovered_again["result"]["structuredContent"]["diagrams"],
        json!([diagram])
    );
    let history = sulion::plans::events(&h.state.pool, id.parse().unwrap())
        .await
        .unwrap();
    let recorded = history
        .iter()
        .find(|event| event.authoring_revision == Some(1))
        .and_then(|event| event.authoring_change.as_ref())
        .unwrap();
    assert_eq!(recorded["client"], "mcp");
    assert_eq!(recorded["subject"], "fixture-user");
    let creation = history
        .iter()
        .find(|event| event.event_type == "plan_created")
        .unwrap();
    assert_eq!(
        creation.authoring_change.as_ref().unwrap()["subject"],
        "fixture-user"
    );
    server.abort();
    issuer_task.abort();
    restore_env("SULION_SECRET_BROKER_URL", broker_url);
    restore_env("SULION_SECRET_BROKER_REGISTRATION_TOKEN", broker_token);
}

async fn verify_terminal_handoff(h: &Harness, id: &str, expected: &Value) {
    let directory = tempfile::tempdir().unwrap();
    let pty = insert_test_pty(&h.state.pool, "alpha", directory.path()).await;
    let socket = directory.path().join("correlate.sock");
    let server = tokio::spawn(sulion::correlate::run(h.state.pool.clone(), socket.clone()));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !socket.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let cli = |args: &[&str]| {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_sulion"));
        command
            .args(["plan", "--json"])
            .args(args)
            .env("SULION_PTY_ID", pty.to_string())
            .env("SULION_CORRELATE_SOCK", &socket);
        async move {
            let output = command.output().await.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice::<Value>(&output.stdout).unwrap()
        }
    };
    cli(&["attach", id]).await;
    let resumed = cli(&["current"]).await;
    assert_eq!(resumed["id"], id);
    assert_eq!(resumed["authoring"]["revision"], 3);
    assert_eq!(resumed["authoring"], expected["authoring"]);
    assert_eq!(resumed["phases"], expected["phases"]);
    let example = expected["authoring"]["entries"][0]["id"].as_str().unwrap();
    let saved = cli(&[
        "entry",
        "set",
        example,
        "--text",
        "An inn can seed development on a suitable site",
    ])
    .await;
    assert_eq!(saved["authoring"]["entries"][0]["id"], example);
    assert_eq!(
        saved["authoring"]["entries"][0]["generalization"],
        expected["authoring"]["entries"][0]["generalization"]
    );
    assert_eq!(
        saved["authoring"]["phases"],
        expected["authoring"]["phases"]
    );
    assert_eq!(saved["authoring"]["revision"], 4);
    let phase = expected["phases"][0]["id"].as_str().unwrap();
    let revised = cli(&[
        "phase",
        "edit",
        phase,
        "--implementation",
        "Extend the current producer and consume the field",
    ])
    .await;
    let detail = &revised["authoring"]["phases"][phase];
    assert_eq!(
        detail["acceptance"],
        expected["authoring"]["phases"][phase]["acceptance"]
    );
    assert_eq!(
        detail["implementation"],
        "Extend the current producer and consume the field"
    );
    server.abort();
}
