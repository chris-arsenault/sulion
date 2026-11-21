use serde_json::Value;

use super::guidance;

pub(crate) fn print_plan_data(data: &Value, full_guidance: bool) {
    if let Some(walkthrough) = data.get("walkthrough").and_then(Value::as_array) {
        print_plan_data(&data["plan"], true);
        if let Some(phase) = data["focus_phase"].as_str() {
            println!("Review focus: phase {phase}");
        }
        println!("Guided plan review");
        for field in ["hypothesis", "example"] {
            if let Some(text) = data["guidance"][field].as_str() {
                println!("  {field}: {text}");
            }
        }
        for (index, step) in walkthrough.iter().enumerate() {
            println!("{}. {}", index + 1, step.as_str().unwrap_or(""));
        }
        for link in data["pattern_references"].as_array().into_iter().flatten() {
            println!("{}", link.as_str().unwrap_or(""));
        }
        return;
    }
    if let Some(items) = data.as_array() {
        print_plan_list(items);
        return;
    }
    if data.get("event_type").is_some() {
        println!(
            "{}",
            serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
        );
        return;
    }
    print_plan_header(data);
    if full_guidance {
        print!("{}", guidance::format_context(data));
        if let Some(authoring) = data.get("authoring") {
            println!("  Authoring revision {}", authoring["revision"]);
            if let Some(details) = authoring["technical_details"]
                .as_str()
                .filter(|text| !text.is_empty())
            {
                println!("  Technical details: {details}");
            }
            if let Some(entries) = authoring["entries"].as_array() {
                for entry in entries.iter().filter(|entry| entry["retired"] != true) {
                    print_entry(entry);
                }
                for (kind, hint) in [
                    ("hypothesis", crate::plans::review::HYPOTHESIS),
                    ("example", crate::plans::review::EXAMPLE),
                ] {
                    if entries
                        .iter()
                        .any(|entry| entry["kind"] == kind && entry["retired"] != true)
                    {
                        println!("    {kind}: {hint}");
                    }
                }
            }
        }
    }
    let branches = data
        .get("branches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if let Some(phases) = data.get("phases").and_then(Value::as_array) {
        for phase in phases {
            println!(
                "  {}. [{}] {}{}",
                phase["position"].as_i64().unwrap_or_default(),
                phase["status"].as_str().unwrap_or("unknown"),
                phase["title"].as_str().unwrap_or(""),
                phase["status_note"]
                    .as_str()
                    .map(|note| format!(" — {note}"))
                    .unwrap_or_default()
            );
            let phase_id = phase["id"].as_str().unwrap_or("");
            if full_guidance {
                println!("     ID {phase_id}");
                if let Some(description) = phase["description"]
                    .as_str()
                    .filter(|text| !text.is_empty())
                {
                    println!("     {description}");
                }
                let details = &data["authoring"]["phases"][phase_id];
                for field in ["implementation", "acceptance"] {
                    if let Some(text) = details[field].as_str().filter(|text| !text.is_empty()) {
                        println!("     {field}: {text}");
                    }
                }
                if let Some(entries) = data["authoring"]["needs_review"][phase_id].as_array() {
                    println!(
                        "     Needs review: {}",
                        entries
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            for branch in branches.iter().filter(|branch| {
                branch["anchor_phase_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(phase_id)))
            }) {
                println!(
                    "     ↳ {} [{}] {} ({}/{} phases)",
                    branch["id"].as_str().unwrap_or(""),
                    branch["status"].as_str().unwrap_or("unknown"),
                    branch["title"].as_str().unwrap_or(""),
                    branch["completed_phases"].as_i64().unwrap_or_default(),
                    branch["total_phases"].as_i64().unwrap_or_default(),
                );
            }
        }
    }
}

fn print_entry(entry: &Value) {
    println!(
        "    [{}] {} {}",
        entry["kind"].as_str().unwrap_or("entry"),
        entry["id"].as_str().unwrap_or(""),
        entry["text"].as_str().unwrap_or("")
    );
    for field in [
        "test",
        "result",
        "evidence",
        "generalization",
        "source",
        "rationale",
    ] {
        if let Some(text) = entry[field].as_str().filter(|text| !text.is_empty()) {
            println!("      {field}: {text}");
        }
    }
}

fn print_plan_header(plan: &Value) {
    // A branch leads with where it sits, so a resumed session sees the trail
    // back to the root before it sees this plan's own phases.
    if let Some(ancestors) = plan
        .get("ancestors")
        .and_then(Value::as_array)
        .filter(|value| !value.is_empty())
    {
        let trail: Vec<&str> = ancestors
            .iter()
            .map(|node| node["title"].as_str().unwrap_or(""))
            .chain(std::iter::once(plan["title"].as_str().unwrap_or("")))
            .collect();
        println!(
            "branch depth {}: {}",
            plan["depth"].as_i64().unwrap_or(0),
            trail.join(" › ")
        );
    }
    println!(
        "{} [{}] {}",
        plan["id"].as_str().unwrap_or(""),
        plan["status"].as_str().unwrap_or("unknown"),
        plan["title"].as_str().unwrap_or("")
    );
    if let Some(summary) = plan["summary"].as_str().filter(|value| !value.is_empty()) {
        println!("  {summary}");
    }
}

fn print_plan_list(items: &[Value]) {
    if items.is_empty() {
        println!("No plans.");
        return;
    }
    if items[0].get("event_type").is_some() {
        for event in items {
            println!(
                "{} {}{}",
                event["created_at"].as_str().unwrap_or(""),
                event["event_type"].as_str().unwrap_or("event"),
                event["note"]
                    .as_str()
                    .map(|note| format!(" — {note}"))
                    .unwrap_or_default()
            );
            if let (Some(from), Some(to)) =
                (event["from_title"].as_str(), event["to_title"].as_str())
            {
                println!("  renamed \"{from}\" → \"{to}\"");
            }
            for key in ["guidance_before", "guidance_after"] {
                if event.get(key).is_some_and(|value| !value.is_null()) {
                    println!("  {}", key.replace('_', " "));
                    print!("{}", guidance::format_fields(&event[key]));
                }
            }
            if let Some(change) = event.get("authoring_change") {
                println!("  authoring revision {}", event["authoring_revision"]);
                println!(
                    "{}",
                    serde_json::to_string_pretty(change).unwrap_or_else(|_| change.to_string())
                );
            }
        }
        return;
    }
    // `plan tree` nodes carry a depth but no summary; indent them instead.
    if items[0].get("depth").is_some() && items[0].get("summary").is_none() {
        for node in items {
            let depth = node["depth"].as_i64().unwrap_or_default().max(0) as usize;
            let attached = node["attached_pty_ids"]
                .as_array()
                .map(|ids| ids.len())
                .unwrap_or_default();
            println!(
                "{}{} [{}] {} ({}/{} phases{}{})",
                "  ".repeat(depth),
                node["id"].as_str().unwrap_or(""),
                node["status"].as_str().unwrap_or("unknown"),
                node["title"].as_str().unwrap_or(""),
                node["completed_phases"].as_i64().unwrap_or_default(),
                node["total_phases"].as_i64().unwrap_or_default(),
                match node["blocked_phases"].as_i64().unwrap_or_default() {
                    0 => String::new(),
                    n => format!(", {n} blocked"),
                },
                if attached > 0 { ", attached" } else { "" },
            );
        }
        return;
    }
    for item in items {
        print_plan_header(item);
    }
}
