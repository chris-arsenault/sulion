use super::{take_flag, take_option, PlanGuidance, Value};
use crate::plans::{BoundarySource, PlanBoundary};

pub(super) fn parse_boundaries(
    args: &mut Vec<String>,
) -> anyhow::Result<Option<Vec<PlanBoundary>>> {
    parse_items(args, "--boundary", "--clear-boundaries")?
        .map(|items| {
            items
                .into_iter()
                .map(|item| {
                    let (source, text) = item.split_once('|').ok_or_else(|| {
                        anyhow::anyhow!(
                "--boundary requires source|text (requirement, correction, or proposal)"
            )
                    })?;
                    let source = match source.trim() {
                        "requirement" => BoundarySource::UserRequirement,
                        "correction" => BoundarySource::UserCorrection,
                        "proposal" => BoundarySource::AgentProposal,
                        _ => anyhow::bail!(
                            "boundary source must be requirement, correction, or proposal"
                        ),
                    };
                    Ok(PlanBoundary {
                        source,
                        text: text.to_string(),
                    })
                })
                .collect()
        })
        .transpose()
}

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
        boundaries: parse_boundaries(args)?.unwrap_or_default(),
    })
}

pub(super) fn format_fields(value: &Value) -> String {
    let mut text = String::new();
    if let Some(outcome) = value["outcome"].as_str().filter(|text| !text.is_empty()) {
        text.push_str(&format!("    Outcome: {outcome}\n"));
    }
    text.push_str(&format_boundaries(value));
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

fn format_boundaries(value: &Value) -> String {
    let mut text = String::new();
    if let Some(items) = value["boundaries"]
        .as_array()
        .filter(|items| !items.is_empty())
    {
        text.push_str("    Boundaries:\n");
        for item in items {
            let source = match item["source"].as_str().unwrap_or("") {
                "user_requirement" => "User requirement",
                "user_correction" => "User correction",
                "agent_proposal" => "Agent proposal",
                _ => "Unspecified source",
            };
            text.push_str(&format!(
                "      - [{source}] {}\n",
                item["text"].as_str().unwrap_or("")
            ));
        }
    }
    text
}

/// Only the working contract recurs on progress commands; detailed guidance
/// remains on current/show and in history.
pub(super) fn format_reminder(plan: &Value) -> String {
    let mut text = String::from(
        "Plan reminder — check the next action against this outcome and these boundaries:\n",
    );
    let ancestors = plan["ancestors"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    for source in ancestors.iter().chain(std::iter::once(plan)) {
        text.push_str(&format!(
            "  {} ({}, revision {}, {}):\n",
            source["title"].as_str().unwrap_or(""),
            source["id"].as_str().unwrap_or(""),
            source["revision"].as_i64().unwrap_or_default(),
            source["status"].as_str().unwrap_or("unknown"),
        ));
        if let Some(outcome) = source["outcome"].as_str().filter(|s| !s.is_empty()) {
            text.push_str(&format!("    Outcome: {outcome}\n"));
        } else {
            text.push_str("    Outcome: not set.\n");
        }
        let boundaries = format_boundaries(source);
        if boundaries.is_empty() {
            text.push_str("    Boundaries: not set. Existing user instructions still apply.\n");
        } else {
            text.push_str(&boundaries);
        }
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
                "--boundary",
                "requirement|No backend",
                "--boundary",
                "correction|Shared interpolation is not generation ownership",
                "--boundary",
                "proposal|Demonstrate one causal path first",
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
            assert_eq!(guidance.boundaries.len(), 3);
            assert_eq!(
                guidance.boundaries[0].source,
                BoundarySource::UserRequirement
            );
            assert_eq!(
                guidance.boundaries[1].source,
                BoundarySource::UserCorrection
            );
            assert_eq!(guidance.boundaries[2].source, BoundarySource::AgentProposal);
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

    #[test]
    fn boundary_updates_preserve_source_and_distinguish_omission_from_clearing() {
        let legacy: PlanGuidance = serde_json::from_value(json!({
            "outcome": "Legacy plan", "principles": [], "assumptions": []
        }))
        .unwrap();
        assert!(legacy.boundaries.is_empty());
        for (args, expected) in [
            (vec![], None),
            (vec!["--clear-boundaries"], Some(vec![])),
            (
                vec![
                    "--boundary",
                    "correction|Do not substitute a shared interpolator",
                ],
                Some(vec![PlanBoundary {
                    source: BoundarySource::UserCorrection,
                    text: "Do not substitute a shared interpolator".into(),
                }]),
            ),
        ] {
            let mut args = args.into_iter().map(str::to_string).collect();
            let ControlRequest::PlanUpdate { input, .. } =
                parse_plan_request("update", &mut args).unwrap()
            else {
                panic!("update")
            };
            assert_eq!(input.boundaries, expected);
        }
        for args in [
            vec!["--boundary", "No source"],
            vec!["--boundary", "guess|A limit"],
            vec!["--boundary", "requirement|A limit", "--clear-boundaries"],
        ] {
            assert!(parse_boundaries(&mut args.into_iter().map(str::to_string).collect()).is_err());
        }
    }

    #[test]
    fn recurring_reminder_preserves_rejected_substitutes_without_full_guidance() {
        let plan = json!({
            "id": "branch", "title": "Terrain fitting", "revision": 1, "status": "active",
            "outcome": "Connect mountain passes", "principles": ["Detail only"],
            "boundaries": [{"source": "agent_proposal", "text": "Demonstrate one range first"}],
            "ancestors": [{"id": "root", "title": "Landscape", "revision": 4, "status": "active",
                "outcome": "Compose interesting landscapes",
                "boundaries": [{"source": "user_correction", "text": "Smooth curves alone do not establish success"}]}]
        });
        let text = format_reminder(&plan);
        assert!(text.contains("[User correction] Smooth curves alone do not establish success"));
        assert!(text.contains("[Agent proposal] Demonstrate one range first"));
        assert!(
            text.find("Compose interesting landscapes").unwrap()
                < text.find("Connect mountain passes").unwrap()
        );
        assert!(!text.contains("Detail only"));
        assert!(!text.contains("Principles"));
    }
}
