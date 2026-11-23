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
    let instructions = initialize["result"]["instructions"].as_str().unwrap();
    assert!(instructions.contains("numbered phases and entries"));
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
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "update_plan_entry"));
    assert!(!tools.iter().any(|tool| tool["name"] == "save_plan"));
    let schema =
        |name: &str| &tools.iter().find(|tool| tool["name"] == name).unwrap()["inputSchema"];
    assert!(!schema("update_plan_phase")["required"]
        .as_array()
        .unwrap()
        .contains(&json!("revision")));
    let required = schema("patch_plan")["required"].as_array().unwrap();
    assert_eq!(required.len(), 2);
    assert!(required.contains(&json!("plan")) && required.contains(&json!("edits")));
    assert!(schema("patch_plan")["properties"]
        .get("operations")
        .is_none());
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
    let read = rpc(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "read_plan", "arguments": {"repo":"second-repo","plan":"Second project"}}})).await;
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
        "plan": "City design", "phase": 1, "implementation": "Extend the current field producer",
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
    let edit_batch = |edits: Value| {
        rpc(
            json!({"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"patch_plan","arguments":{"plan":"Second project","edits":edits}}}),
        )
    };
    let rejected = edit_batch(json!([
        "update --summary 'Uncommitted'",
        "entry set 9 --text 'Missing'"
    ]))
    .await;
    assert_eq!(rejected["result"]["isError"], true, "{rejected}");
    let applied = edit_batch(json!([
        "entry add decision 'Product files'",
        "entry set 1 --rationale 'Git diff'",
        "phase edit 1 --basis 1 --acceptance 'Diff the document'"
    ]))
    .await;
    let updated = &applied["result"]["structuredContent"]["plan"];
    assert_eq!(updated["summary"], "Edited from MCP", "{applied}");
    assert_eq!(updated["authoring"]["entries"][0]["number"], 1);
    assert_eq!(updated["authoring"]["entries"][0]["rationale"], "Git diff");
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
    let mcp_url = std::env::var_os("SULION_PLAN_MCP_URL");
    std::env::set_var("SULION_PLAN_MCP_URL", "https://sulion.test/mcp");
    let terminal_diagram =
        verify_terminal_handoff(&h, id, &read, &reviewed["plan"]["phases"]).await;
    restore_env("SULION_PLAN_MCP_URL", mcp_url);
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
        json!([diagram, terminal_diagram])
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
        json!([diagram, terminal_diagram])
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
    let upload = tools.iter().find(|t| t["name"] == "upload_file").unwrap();
    let delete = tools
        .iter()
        .find(|t| t["name"] == "delete_data_set")
        .unwrap();
    assert_eq!(delete["annotations"]["destructiveHint"], true);
    assert_eq!(delete["annotations"]["readOnlyHint"], false);
    assert_eq!(upload["_meta"]["openai/fileParams"], json!(["file"]));
    let file_schema = &upload["inputSchema"]["$defs"]["ChatFile"];
    for field in ["file_id", "download_url", "file_name", "mime_type"] {
        assert!(
            file_schema["properties"].get(field).is_some(),
            "{file_schema}"
        );
    }
    assert_eq!(file_schema["required"].as_array().unwrap().len(), 2);
    let oversize = request()
        .bearer_auth(&token)
        .json(&json!({"jsonrpc":"2.0","id":21,
        "method":"tools/call","params":{"name":"create_plan","arguments":{"repo":"alpha",
        "title":"must not be created","summary":"x".repeat(256 * 1024),"phases":[]}}}))
        .send()
        .await
        .unwrap();
    assert_eq!(oversize.status(), 413);
    let edits: Vec<_> = (0..12)
        .map(|_| format!("phase edit 1 --implementation '{}'", "x".repeat(6000)))
        .collect();
    let larger = call("patch_plan", json!({"plan":"City design","edits":edits})).await;
    assert_eq!(larger["result"]["isError"], false, "{larger}");
    verify_documents_and_transfer(&h, call).await;
    server.abort();
    issuer_task.abort();
    restore_env("SULION_SECRET_BROKER_URL", broker_url);
    restore_env("SULION_SECRET_BROKER_REGISTRATION_TOKEN", broker_token);
}

async fn verify_documents_and_transfer<F, R>(h: &Harness, rpc: F)
where
    F: Fn(&str, Value) -> R,
    R: std::future::Future<Output = Value>,
{
    let root = h.repos_root().join("product");
    std::fs::create_dir(&root).unwrap();
    let path = "docs/process.dataset.json";
    let doc = |mut args: Value| {
        args["repo"] = json!("product");
        args["path"] = json!(path);
        args
    };
    let success = |reply: Value| {
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        reply["result"]["structuredContent"].clone()
    };
    let schema = json!({"keys":["process","map"],"fields":{
        "process":{"type":"string"},"map":{"type":"string","allowed":["World","Region"]},
        "strategy":{"type":"string"},"priority":{"type":"integer"}}});
    success(rpc("define_data_set", doc(json!({"schema":schema}))).await);
    success(
        rpc(
            "fill_data_set_items",
            doc(json!({"items":[{"process":"water","map":"Region"},
        {"process":"water","map":"World","strategy":"x".repeat(70_000)}]})),
        )
        .await,
    );
    assert_eq!(
        success(rpc("list_data_sets", json!({"repo":"product"})).await)["paths"],
        json!([path])
    );
    let gaps = success(rpc("read_data_set", doc(json!({"missing":["strategy"]}))).await);
    assert_eq!(gaps["total"], 1);
    let stale = gaps["revisions"][0].clone();
    let current = std::fs::read_to_string(root.join(path)).unwrap();
    let mut document: Value = serde_json::from_str(&current).unwrap();
    document["items"][0]["strategy"] = json!("An editor's manual change");
    std::fs::write(
        root.join(path),
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    let reject = rpc(
        "fill_data_set_items",
        doc(
            json!({"expected":[{"key":{"process":"water","map":"Region"},"revision":stale}],
        "items":[{"process":"water","map":"Region","priority":3}]}),
        ),
    )
    .await;
    assert_eq!(reject["result"]["isError"], true);
    let failed = rpc(
        "fill_data_set_items",
        doc(
            json!({"items":[{"process":"water","map":"World","priority":2},
        {"process":"water","map":"Region","priority":"urgent"}]}),
        ),
    )
    .await;
    assert_eq!(failed["result"]["isError"], true);
    let saved = success(
        rpc(
            "read_data_set",
            doc(json!({"where":{"map":"World"},"columns":["priority"]})),
        )
        .await,
    );
    assert!(saved["items"][0].get("priority").is_none());
    success(
        rpc(
            "import_data_set",
            doc(json!({"format":"csv","data":"process,map,priority\nwater,Region,3\n"})),
        )
        .await,
    );
    let saved = success(
        rpc(
            "export_data_set",
            doc(json!({"format":"jsonl","where":{"map":"Region"}})),
        )
        .await,
    );
    let row: Value = serde_json::from_str(saved["data"].as_str().unwrap()).unwrap();
    assert_eq!(row["strategy"], "An editor's manual change");
    assert_eq!(row["priority"], 3);
    let dataset_cli = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sulion"))
            .arg("dataset")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(dataset_cli(&["read", path, "--where", "map=Region"]).contains("priority: 3"));
    dataset_cli(&[
        "set",
        path,
        "map=Region",
        "process=water",
        "priority=4",
        "--row",
        "map=World",
        "process=water",
        "strategy",
    ]);
    let cross_read = success(rpc("read_data_set", doc(json!({"missing":["strategy"]}))).await);
    assert_eq!(
        cross_read["items"],
        json!([{"process":"water","map":"World"}])
    );
    let csv = dataset_cli(&[
        "export",
        path,
        "--format",
        "csv",
        "--where",
        "map=Region",
        "--column",
        "priority",
    ]);
    assert_eq!(
        csv.lines().collect::<Vec<_>>(),
        ["process,map,priority", "water,Region,4"]
    );
    let deleted = success(rpc("delete_data_set", doc(json!({}))).await);
    assert_eq!(deleted, json!({"path":path,"deleted":true}));
    assert!(dataset_cli(&["list"]).is_empty());
    success(rpc("define_data_set", doc(json!({"schema":schema}))).await);
    dataset_cli(&["delete", path]);
    assert_eq!(
        success(rpc("list_data_sets", json!({"repo":"product"})).await)["paths"],
        json!([])
    );
    assert!(h
        .state
        .upload_store
        .set(sulion::uploads::store::StagingStore::for_test())
        .is_ok());
    let grant = success(
        rpc(
            "begin_file_upload",
            json!({"repo":"product","directory":"docs","filename":"table.csv",
        "size":65536,"checksum":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}),
        )
        .await,
    );
    assert!(grant["grant"]["url"]
        .as_str()
        .unwrap()
        .contains("X-Amz-Signature"));
    let refused = rpc(
        "upload_file",
        json!({"repo":"product","file":{"file_name":"a.csv","file_id":"fixture",
        "download_url":"https://example.org/file"}}),
    )
    .await;
    assert_eq!(refused["result"]["isError"], true);
}

#[test]
fn terminal_dataset_batches_and_filtered_exports_preserve_requirements() {
    let checkout = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let run = |command: &str, args: &[&str], ok: bool| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sulion"))
            .current_dir(checkout.path())
            .arg(command)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            ok,
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let path = "docs/process.dataset.json";
    run(
        "dataset",
        &[
            "define",
            path,
            "--key",
            "Process ID",
            "--field",
            "Process ID=string",
            "--field",
            "Priority=integer",
            "--field",
            "Map=string",
            "--allow",
            "Map=World",
        ],
        true,
    );
    let input = source.path().join("rows.csv");
    let mut csv = String::from("Process ID,Priority,Map\n");
    for i in 0..30 {
        csv.push_str(&format!("{i:03},1,World\n"));
    }
    std::fs::write(&input, csv).unwrap();
    run(
        "upload",
        &[input.to_str().unwrap(), "docs/source.csv"],
        true,
    );
    run("dataset", &["import", path, "docs/source.csv"], true);
    let exported = run(
        "dataset",
        &[
            "export",
            path,
            "--format",
            "csv",
            "--where",
            "Map=World",
            "--column",
            "Priority",
        ],
        true,
    );
    assert_eq!(exported.lines().count(), 31);
    assert!(exported.starts_with("Process ID,Priority\r\n000,1\r\n"));
    let baseline = run(
        "dataset",
        &["read", path, "--where", "Process ID=001", "--json"],
        true,
    );
    std::fs::write(checkout.path().join("baseline.json"), baseline).unwrap();
    run(
        "dataset",
        &[
            "set",
            path,
            "Process ID=001",
            "Priority=2",
            "--if-unchanged",
            "baseline.json",
        ],
        true,
    );
    let before = std::fs::read(checkout.path().join(path)).unwrap();
    run(
        "dataset",
        &[
            "set",
            path,
            "Process ID=001",
            "Priority=3",
            "--if-unchanged",
            "baseline.json",
        ],
        false,
    );
    run(
        "dataset",
        &[
            "set",
            path,
            "Process ID=001",
            "Priority=3",
            "--row",
            "Process ID=002",
            "Map=Ocean",
        ],
        false,
    );
    assert_eq!(before, std::fs::read(checkout.path().join(path)).unwrap());
    let link = "linked.dataset.json";
    std::os::unix::fs::symlink(checkout.path().join(path), checkout.path().join(link)).unwrap();
    run("dataset", &["delete", link], false);
    run("dataset", &["delete", "../outside.dataset.json"], false);
    run("dataset", &["delete", "docs/source.csv"], false);
    assert_eq!(before, std::fs::read(checkout.path().join(path)).unwrap());
    run("dataset", &["delete", path], true);
    run("dataset", &["delete", path], false);
    assert!(!checkout.path().join(path).exists());
    assert!(checkout.path().join("docs/source.csv").is_file());
}

async fn verify_terminal_handoff(
    h: &Harness,
    id: &str,
    expected: &Value,
    mcp_phases: &Value,
) -> Value {
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
    cli(&["attach", "City design"]).await;
    let resumed = cli(&["current"]).await;
    assert_eq!(resumed["id"], id);
    assert_eq!(resumed["authoring"]["revision"], 3);
    assert_eq!(resumed["authoring"], expected["authoring"]);
    assert_eq!(&resumed["phases"], mcp_phases);
    let example = expected["authoring"]["entries"][0]["id"].as_str().unwrap();
    let saved = cli(&[
        "entry",
        "set",
        "1",
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
    let workspace = directory.path().join("workspace.json");
    let room = "b".repeat(48);
    let diagram = json!({"workspace_id":Uuid::new_v4(),"room_id":room,"title":"Road network"});
    std::fs::write(&workspace, json!({"structuredContent":diagram}).to_string()).unwrap();
    let registered = cli(&["diagram", "register", workspace.to_str().unwrap()]).await;
    let url = registered["diagram"]["url"].as_str().unwrap();
    let registry = cli(&["diagram", "list"]).await;
    assert!(registry["diagrams"]
        .as_array()
        .unwrap()
        .contains(&registered["diagram"]));
    let linked = cli(&[
        "diagram",
        "attach",
        "Road network",
        "--link",
        &format!("1={url}&item=roads"),
    ])
    .await;
    assert_eq!(linked["diagrams"][0]["phase_links"][0]["phase_id"], phase);
    let detached = cli(&["diagram", "detach", url]).await;
    assert!(detached.get("diagrams").is_none());
    server.abort();
    registered["diagram"].clone()
}
