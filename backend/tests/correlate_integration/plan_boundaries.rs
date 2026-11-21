use super::*;
use serde_json::Value;

async fn cli(sock: &Path, pty: Uuid, args: &[&str], code: i32) -> (String, String) {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_sulion"))
            .arg("plan")
            .args(args)
            .env("SULION_PTY_ID", pty.to_string())
            .env("SULION_CORRELATE_SOCK", sock)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "{args:?}\n{stdout}\n{stderr}"
    );
    (stdout, stderr)
}

#[tokio::test]
async fn every_plan_command_recovers_boundaries_and_preserves_json_output() {
    let pool = fresh_pool().await;
    let pty = Uuid::new_v4();
    sqlx::query("INSERT INTO pty_sessions (id, repo, working_dir, state, created_at) VALUES ($1, 'bounds', '/tmp', 'live', NOW())")
        .bind(pty).execute(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let listener_pool = pool.clone();
    let listener_sock = sock.clone();
    let listener = tokio::spawn(async move { correlate::run(listener_pool, listener_sock).await });
    wait_for_socket(&sock).await;

    let (out, err) = cli(
        &sock,
        pty,
        &[
            "--json",
            "start",
            "Landscape",
            "--phase",
            "Compose",
            "--outcome",
            "Compose interesting landscapes",
            "--boundary",
            "requirement|Browser only",
            "--boundary",
            "correction|Smooth curves alone do not establish success",
        ],
        0,
    )
    .await;
    let root: Value = serde_json::from_str(&out).unwrap();
    let root_id = root["id"].as_str().unwrap();
    assert!(err.contains("[User requirement] Browser only"));
    assert!(err.contains("[User correction] Smooth curves alone do not establish success"));

    for args in [
        vec!["help"],
        vec!["current"],
        vec!["show", root_id],
        vec!["list"],
        vec!["history", root_id],
        vec!["tree", root_id],
        vec!["status", "paused"],
        vec!["status", "active"],
        vec!["update", "--summary", "Refined scope"],
        vec!["phase", "add", "Verify"],
        vec!["step", "set", "2", "completed"],
    ] {
        let (out, _) = cli(&sock, pty, &args, 0).await;
        assert!(
            out.contains("Compose interesting landscapes"),
            "{args:?}: {out}"
        );
        assert!(
            out.contains("[User correction] Smooth curves alone do not establish success"),
            "{args:?}: {out}"
        );
    }
    for (args, code) in [
        (vec!["close", "--completed"], 66),
        (vec!["update", "--bogus"], 64),
        (vec![], 64),
    ] {
        let (out, err) = cli(&sock, pty, &args, code).await;
        assert!(format!("{out}{err}").contains("[User requirement] Browser only"));
    }
    let (out, _) = cli(
        &sock,
        pty,
        &[
            "--json",
            "branch",
            "Terrain",
            "--phase",
            "Fit",
            "--outcome",
            "Connect passes",
            "--boundary",
            "proposal|Demonstrate one range first",
        ],
        0,
    )
    .await;
    let branch: Value = serde_json::from_str(&out).unwrap();
    let branch_id = branch["id"].as_str().unwrap();
    for args in [
        vec!["current"],
        vec!["list"],
        vec!["tree"],
        vec!["history"],
        vec!["phase", "set", "1", "completed"],
    ] {
        let (out, _) = cli(&sock, pty, &args, 0).await;
        assert!(out.contains("[User requirement] Browser only"));
        assert!(out.contains("[Agent proposal] Demonstrate one range first"));
    }
    // An explicitly requested root uses its own context even while attached to a branch.
    for command in ["history", "tree", "show"] {
        let (out, err) = cli(&sock, pty, &["--json", command, root_id], 0).await;
        serde_json::from_str::<Value>(&out).unwrap();
        assert!(err.contains("Browser only"));
        assert!(!err.contains("Demonstrate one range first"));
    }
    let (out, _) = cli(&sock, pty, &["return", "--completed"], 0).await;
    assert!(out.contains("[User correction] Smooth curves alone do not establish success"));
    let (out, _) = cli(&sock, pty, &["current"], 0).await;
    assert!(!out.contains("[Agent proposal] Demonstrate one range first"));

    let (out, _) = cli(&sock, pty, &["detach"], 0).await;
    assert!(out.contains("Browser only"));
    let (out, _) = cli(&sock, pty, &["attach", root_id], 0).await;
    assert!(out.contains("Browser only"));
    let (out, _) = cli(
        &sock,
        pty,
        &[
            "update",
            "--boundary",
            "correction|No publication-side substitute",
            "--note",
            "User corrected architecture",
        ],
        0,
    )
    .await;
    assert!(out.contains("[User correction] No publication-side substitute"));
    assert!(!out.contains("Browser only"));
    let (out, _) = cli(&sock, pty, &["show", branch_id], 0).await;
    assert!(out.contains("[User correction] No publication-side substitute"));
    let (out, _) = cli(&sock, pty, &["history"], 0).await;
    assert!(out.contains("Browser only") && out.contains("No publication-side substitute"));
    let (out, _) = cli(&sock, pty, &["update", "--clear-boundaries"], 0).await;
    assert!(out.contains("Boundaries: not set. Existing user instructions still apply."));
    let (out, _) = cli(&sock, pty, &["close", "--completed", "--skip-remaining"], 0).await;
    assert!(out.contains("Compose interesting landscapes"));
    let (out, err) = cli(&sock, pty, &["--json", "list"], 0).await;
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap(),
        serde_json::json!([])
    );
    assert!(err.contains("no plan is attached"));
    let (_, err) = cli(&sock, pty, &["help"], 0).await;
    assert!(err.contains("no plan is attached"));
    listener.abort();
}

#[tokio::test]
async fn cli_patches_full_plan_atomically_and_rejects_stale_edits() {
    use serde_json::json;
    let pool = fresh_pool().await;
    let pty = Uuid::new_v4();
    sqlx::query("INSERT INTO pty_sessions (id, repo, working_dir, state) VALUES ($1, 'drafts', '/tmp', 'live')")
        .bind(pty).execute(&pool).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("control.sock");
    let listener_sock = sock.clone();
    let listener = tokio::spawn(async move { correlate::run(pool, listener_sock).await });
    wait_for_socket(&sock).await;
    let (initial, _) = cli(
        &sock,
        pty,
        &[
            "--json",
            "start",
            "City refinement",
            "--phase",
            "Develop city",
            "--all-pending",
            "--boundary",
            "requirement|Keep the existing refinement process",
        ],
        0,
    )
    .await;
    let initial: Value = serde_json::from_str(&initial).unwrap();
    let phase = &initial["phases"][0]["id"];
    let boundary = &initial["authoring"]["entries"][0]["id"];
    let example = Uuid::new_v4();
    let test = Uuid::new_v4();
    let patch = json!({"revision": initial["authoring"]["revision"], "reason": "Clarify the design", "operations": [
        {"op": "put_entry", "id": boundary, "entry": {"kind": "boundary", "attribution": "user_requirement", "text": "Keep the existing refinement path"}},
        {"op": "put_entry", "id": test, "entry": {"kind": "hypothesis", "text": "A seed raises population nearby", "goal": boundary, "test": "Compare the resulting population field to an unseeded cell"}},
        {"op": "put_entry", "id": example, "entry": {"kind": "example", "text": "An inn grows neighboring streets", "source": "user turn", "generalization": "Site conditions choose a seed for city-scale development"}},
        {"op": "edit_phase", "id": phase, "implementation": "Use the owner's people-field operations", "acceptance": "The next refinement round reads the changed fields", "basis": [boundary]}
    ]}).to_string();
    let (edited, _) = cli(&sock, pty, &["--json", "patch", &patch], 0).await;
    let edited: Value = serde_json::from_str(&edited).unwrap();
    assert_eq!(edited["authoring"]["entries"][0]["id"], *boundary);
    assert_eq!(
        edited["boundaries"][0]["text"],
        "Keep the existing refinement path"
    );
    assert_eq!(
        edited["authoring"]["phases"][phase.as_str().unwrap()]["basis"],
        json!([boundary])
    );
    assert_eq!(edited["phases"][0]["status"], "pending");
    assert!(cli(&sock, pty, &["patch", &patch], 66)
        .await
        .1
        .contains("revision conflict"));
    let invalid = json!({"revision": edited["authoring"]["revision"], "operations": [
        {"op": "edit_plan", "summary": "Must roll back"},
        {"op": "edit_phase", "id": Uuid::new_v4(), "implementation": "Missing phase"}
    ]})
    .to_string();
    cli(&sock, pty, &["patch", &invalid], 66).await;
    let (current, _) = cli(&sock, pty, &["--json", "current"], 0).await;
    assert_eq!(serde_json::from_str::<Value>(&current).unwrap(), edited);
    let (history, _) = cli(&sock, pty, &["--json", "history"], 0).await;
    let history: Value = serde_json::from_str(&history).unwrap();
    let changes: Vec<_> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["event_type"] == "authoring_changed")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["authoring_change"]["client"], "pty");
    assert_eq!(
        changes[0]["authoring_change"]["changes"][0]["before"]["text"],
        "Keep the existing refinement process"
    );
    let result = json!({"revision": edited["authoring"]["revision"], "reason": "Record the experiment and clarify the mechanism", "operations": [
        {"op": "put_entry", "id": test, "entry": {"kind": "hypothesis", "text": "A seed raises population nearby", "goal": boundary, "test": "Compare the resulting population field to an unseeded cell", "result": "supported", "evidence": "One seeded cell gained population"}},
        {"op": "put_entry", "id": example, "entry": {"kind": "example", "text": "An inn grows neighboring streets", "source": "user turn", "generalization": "Site conditions and the template choose a seed that affects city-scale fabric and fields consumed by the next round"}},
        {"op": "edit_plan", "summary": "Extend the existing city refinement loop", "technical_details": "Extend the field update in the refinement owner, then pass its output into the existing next-round input."}
    ]}).to_string();
    cli(&sock, pty, &["patch", &result], 0).await;
    let (review, _) = cli(
        &sock,
        pty,
        &["--json", "review", "--phase", phase.as_str().unwrap()],
        0,
    )
    .await;
    let review: Value = serde_json::from_str(&review).unwrap();
    assert_eq!(review["focus_phase"], *phase);
    assert_eq!(review["plan"]["phases"][0]["status"], "pending");
    assert_eq!(
        review["plan"]["authoring"]["entries"][1]["result"],
        "supported"
    );
    assert_eq!(
        review["plan"]["authoring"]["entries"][2]["source"],
        "user turn"
    );
    assert_eq!(
        review["plan"]["authoring"]["phases"],
        edited["authoring"]["phases"]
    );
    listener.abort();
}
