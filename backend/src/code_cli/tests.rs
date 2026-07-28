use super::output::pack_target_for_result;
use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn parses_global_options_around_command() {
    let parsed = CliInvocation::parse(&args(&[
        "outline",
        "backend/src",
        "--budget",
        "small",
        "--json",
    ]))
    .unwrap();

    assert!(parsed.json);
    assert_eq!(parsed.budget, "small");
    assert_eq!(
        parsed.command,
        CodeCommand::Outline {
            path: Some("backend/src".to_string())
        }
    );
}

#[test]
fn rejects_non_canonical_options() {
    let err = CliInvocation::parse(&args(&["status", "--repo", "sulion"])).unwrap_err();

    assert!(err.message.contains("unknown option"));
    assert_eq!(err.next, "sulion-code help");
}

#[test]
fn patch_request_is_diff_only_command_shape() {
    let parsed = CliInvocation::parse(&args(&[
        "patch",
        "rust",
        "foo($A)",
        "bar($A)",
        "backend/src",
        "--budget=large",
    ]))
    .unwrap();
    let request = parsed
        .command
        .request("/home/sulion/repos/sulion", &parsed.budget);

    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/patch");
    assert_eq!(request.query.len(), 0);
    let body = request.body.unwrap();
    assert_eq!(body["cwd"], "/home/sulion/repos/sulion");
    assert_eq!(body["lang"], "rust");
    assert_eq!(body["rewrite"], "bar($A)");
    assert_eq!(body["path"], "backend/src");
    assert_eq!(body["budget"], "large");
}

#[test]
fn help_does_not_require_service_env() {
    let parsed = CliInvocation::parse(&args(&["help", "--json"])).unwrap();

    assert!(parsed.json);
    assert_eq!(parsed.command, CodeCommand::Help);
}

#[test]
fn pack_hint_uses_symbol_id_when_pack_accepts_it() {
    let result = json!({
        "id": "sym_abc123",
        "range": {
            "path": "backend/src/lib.rs",
            "start_line": 7,
            "start_col": 1,
            "end_line": 9,
            "end_col": 2
        }
    });

    assert_eq!(pack_target_for_result(&result), "sym_abc123");
}

#[test]
fn pack_hint_uses_range_for_semantic_result_ids() {
    let result = json!({
        "id": "semantic:frontend/src/components/Sidebar.tsx:59:17",
        "range": {
            "path": "frontend/src/components/Sidebar.tsx",
            "start_line": 59,
            "start_col": 17,
            "end_line": 59,
            "end_col": 24
        }
    });

    assert_eq!(
        pack_target_for_result(&result),
        "frontend/src/components/Sidebar.tsx:59-59"
    );
}

#[test]
fn infers_repo_from_workspace_path() {
    assert_eq!(
        infer_repo("/home/sulion/workspaces/sulion/branch"),
        Some("sulion".to_string())
    );
}
