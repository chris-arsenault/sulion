use super::*;

fn project_command(command: &str) -> (Value, Option<String>, Option<OperationCategory>) {
    let mut input = json!({"command": command});
    let mut kind = Some("bash".into());
    let mut category = Some(OperationCategory::Utility);
    project(&mut input, &mut kind, &mut category);
    (input, kind, category)
}

#[test]
fn uses_cli_argument_grammar_for_phase_and_close_metadata() {
    let (input, kind, category) =
        project_command("sulion plan phase set --note 'Ready; keep tests' --status completed 2");
    assert_eq!(kind.as_deref(), Some("sulion_plan"));
    assert_eq!(category, Some(OperationCategory::Plan));
    assert_eq!(
        input["plan_commands"][0],
        json!({"action":"phase set","phase":"2","status":"completed","note":"Ready; keep tests"})
    );
    let (input, _, _) = project_command("sulion plan close --completed --note done");
    assert_eq!(input["plan_commands"][0]["status"], "completed");
}

#[test]
fn recognizes_wrappers_paths_assignments_and_json() {
    for command in [
        "SULION_REPO_NAME=sulion /opt/sulion/bin/sulion plan --json current",
        "env SULION_REPO_NAME=sulion command sulion plan list --all",
        "with-cred -- sulion plan show",
        "timeout 5 sulion plan history",
        "cd /repo && sulion plan current",
    ] {
        let (input, kind, _) = project_command(command);
        assert_eq!(kind.as_deref(), Some("sulion_plan"), "{command}: {input}");
    }
}

#[test]
fn preserves_title_guidance_and_branch_anchors() {
    let (input, _, _) = project_command("sulion plan branch 'Ship \\\"plans\\\"' --from 2 --from 3 --summary 'Review progress' --phase 'Verify|Tests|m' --outcome 'Visible plans'");
    let metadata = &input["plan_commands"][0];
    assert_eq!(metadata["title"], "Ship \\\"plans\\\"");
    assert_eq!(metadata["from"], json!(["2", "3"]));
    assert_eq!(metadata["phase_count"], 1);
    assert_eq!(metadata["outcome"], "Visible plans");
}

#[test]
fn ignores_quoted_examples_comments_and_uninvoked_functions() {
    for command in [
        "echo 'sulion plan close --completed'",
        "rg 'sulion plan' docs",
        "# sulion plan close --completed\ncat docs/plans.md",
        "f() { sulion plan close --completed; }",
        "cat <<'EOF'\nsulion plan close --completed\nEOF",
    ] {
        let (input, kind, category) = project_command(command);
        assert!(input.get("plan_commands").is_none(), "{command}: {input}");
        assert_eq!(kind.as_deref(), Some("bash"));
        assert_eq!(category, Some(OperationCategory::Utility));
    }
}

#[test]
fn retains_other_operations_in_mixed_shell_and_code_mode_batches() {
    let (input, kind, _) = project_command("sulion plan current && git status --short");
    assert_eq!(kind.as_deref(), Some("bash"));
    assert_eq!(input["plan_commands"][0]["action"], "current");
    let mut input = json!({"command":"sulion plan current && git status", "operations":[
        {"name":"sulion","input":{"cmd":"sulion plan current"}},
        {"name":"git","input":{"cmd":"git status"}}
    ]});
    let original = input["operations"].clone();
    let mut kind = Some("parallel".into());
    let mut category = Some(OperationCategory::Workflow);
    assert!(project(&mut input, &mut kind, &mut category));
    assert_eq!(kind.as_deref(), Some("parallel"));
    assert_eq!(input["operations"], original);
    assert_eq!(input["plan_commands"].as_array().unwrap().len(), 1);
}

#[test]
fn parses_each_plan_invocation_and_preserves_dynamic_arguments_as_raw_evidence() {
    let (input, kind, _) =
        project_command("sulion plan current; sulion plan phase set 1 completed");
    assert_eq!(kind.as_deref(), Some("sulion_plan"));
    assert_eq!(input["plan_commands"].as_array().unwrap().len(), 2);
    let (input, _, _) =
        project_command("sulion plan phase set \"$phase\" completed --note \"$(date)\"");
    assert_eq!(input["plan_commands"][0], json!({"action":"phase set"}));
    let (input, _, _) = project_command("sulion plan \"$action\" close");
    assert_eq!(input["plan_commands"][0], json!({"action":"unknown"}));
    let (input, _, _) = project_command("sulion plan phase \"$action\" completed");
    assert_eq!(input["plan_commands"][0], json!({"action":"phase"}));
}
