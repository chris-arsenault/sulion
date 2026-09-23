use super::{take_flag, take_option, PlanGuidance, Value};

pub(super) fn parse_items(
    args: &mut Vec<String>,
    option: &str,
    clear: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    let clearing = take_flag(args, clear);
    let mut items = Vec::new();
    while let Some(item) = take_option(args, option)? {
        items.push(item);
    }
    if clearing && !items.is_empty() {
        anyhow::bail!("{option} cannot be combined with {clear}");
    }
    Ok((clearing || !items.is_empty()).then_some(items))
}

pub(super) fn parse_new(args: &mut Vec<String>) -> anyhow::Result<PlanGuidance> {
    Ok(PlanGuidance {
        outcome: take_option(args, "--outcome")?.unwrap_or_default(),
        principles: parse_items(args, "--principle", "--clear-principles")?.unwrap_or_default(),
        assumptions: parse_items(args, "--assumption", "--clear-assumptions")?.unwrap_or_default(),
    })
}

pub(super) fn format_fields(value: &Value) -> String {
    let mut text = String::new();
    if let Some(outcome) = value["outcome"].as_str().filter(|text| !text.is_empty()) {
        text.push_str(&format!("    Outcome: {outcome}\n"));
    }
    for (key, label) in [("principles", "Principles"), ("assumptions", "Assumptions")] {
        if let Some(items) = value[key].as_array().filter(|items| !items.is_empty()) {
            text.push_str(&format!("    {label}:\n"));
            for item in items.iter().filter_map(Value::as_str) {
                text.push_str(&format!("      - {item}\n"));
            }
        }
    }
    if text.is_empty() {
        text.push_str("    No guidance set.\n");
    }
    text
}

pub(super) fn format_context(plan: &Value) -> String {
    let mut text = String::new();
    if let Some(ancestors) = plan["ancestors"].as_array() {
        for ancestor in ancestors {
            text.push_str(&format!(
                "  Guidance from {} ({}, revision {}):\n",
                ancestor["title"].as_str().unwrap_or(""),
                ancestor["id"].as_str().unwrap_or(""),
                ancestor["revision"].as_i64().unwrap_or_default(),
            ));
            text.push_str(&format_fields(ancestor));
        }
    }
    text.push_str("  Guidance for this plan:\n");
    text.push_str(&format_fields(plan));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{correlate::ControlRequest, plan_cli::parse_plan_request};
    use serde_json::json;

    #[test]
    fn start_and_branch_accept_ordered_guidance() {
        for command in ["start", "branch"] {
            let mut args = [
                "Work",
                "--phase",
                "Build",
                "--outcome",
                "Benefit",
                "--principle",
                "First",
                "--principle",
                "Second",
                "--assumption",
                "Unverified",
            ]
            .map(str::to_string)
            .to_vec();
            let request = parse_plan_request(command, &mut args).unwrap();
            let guidance = match request {
                ControlRequest::PlanStart { guidance, .. } => guidance,
                ControlRequest::PlanBranch { input, .. } => input.guidance,
                _ => panic!("expected creation"),
            };
            assert_eq!(guidance.outcome, "Benefit");
            assert_eq!(guidance.principles, ["First", "Second"]);
            assert_eq!(guidance.assumptions, ["Unverified"]);
        }
    }

    #[test]
    fn updates_distinguish_omitted_replaced_and_cleared_fields() {
        let mut args = ["--principle", "Replacement", "--clear-assumptions"]
            .map(str::to_string)
            .to_vec();
        let ControlRequest::PlanUpdate { input, .. } =
            parse_plan_request("update", &mut args).unwrap()
        else {
            panic!("expected update");
        };
        assert_eq!(input.outcome, None);
        assert_eq!(input.principles, Some(vec!["Replacement".into()]));
        assert_eq!(input.assumptions, Some(vec![]));
        let mut args = ["--principle", "Keep", "--clear-principles"]
            .map(str::to_string)
            .to_vec();
        assert!(parse_plan_request("update", &mut args).is_err());
    }

    #[test]
    fn context_labels_sources_and_keeps_ancestor_order() {
        let text = format_context(&json!({
            "outcome": "Branch outcome", "principles": [], "assumptions": [],
            "ancestors": [
                {"id": "root", "title": "Root", "revision": 3, "outcome": "Root outcome"},
                {"id": "parent", "title": "Parent", "revision": 2, "principles": ["Preserve intent"]}
            ]
        }));
        assert!(text.contains("Guidance from Root (root, revision 3)"));
        assert!(text.contains("Guidance from Parent (parent, revision 2)"));
        assert!(text.find("Root outcome").unwrap() < text.find("Preserve intent").unwrap());
        assert!(text.find("Preserve intent").unwrap() < text.find("Branch outcome").unwrap());
    }
}
