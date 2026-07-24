use super::CodeCommand;
use serde_json::Value;

pub(super) fn print_text(command: &CodeCommand, body: &Value) {
    match command {
        CodeCommand::Status => print_status(body),
        CodeCommand::IndexStatus => print_index_status(body),
        CodeCommand::Refresh { .. } => print_refresh(body),
        CodeCommand::Outline { .. } | CodeCommand::Find { .. } | CodeCommand::Def { .. } => {
            print_symbol_results(body)
        }
        CodeCommand::Refs { .. } => print_reference_results(body),
        CodeCommand::Search { .. } => print_search_results(body),
        CodeCommand::Patch { .. } => print_patch(body),
        CodeCommand::Pack { .. } => print_pack(body),
        CodeCommand::Help => unreachable!("help is handled locally"),
    }
}

fn print_index_status(body: &Value) {
    print_warnings(body);
    println!(
        "root: {} {} {}",
        text(&body["root"], "kind"),
        text(&body["root"], "name"),
        text(&body["root"], "path")
    );
    println!(
        "state: freshness={} confidence={}",
        text(body, "freshness"),
        text(body, "confidence")
    );
    print_index_summary(&body["index"]);
    let latest_job = &body["index"]["latest_job"];
    if latest_job.is_object() {
        println!(
            "latest-job: status={} trigger={} seen={} indexed={} failed={}",
            text(latest_job, "status"),
            text(latest_job, "trigger"),
            number(latest_job, "files_seen"),
            number(latest_job, "files_indexed"),
            number(latest_job, "files_failed")
        );
    }
}

fn print_status(body: &Value) {
    print_warnings(body);
    println!(
        "root: {} {} {}",
        text(&body["root"], "kind"),
        text(&body["root"], "name"),
        text(&body["root"], "path")
    );
    println!(
        "state: freshness={} confidence={}",
        text(body, "freshness"),
        text(body, "confidence")
    );
    print_index_summary(&body["index"]);
    let semantic = &body["semantic"];
    println!(
        "semantic: available={} active_servers={}/{} fallback={} timeout_ms={} warmup_timeout_ms={} idle_timeout_ms={}",
        bool_text(semantic, "available"),
        number(semantic, "active_servers"),
        number(semantic, "max_active_servers"),
        text(semantic, "fallback"),
        number(semantic, "timeout_ms"),
        number(semantic, "warmup_timeout_ms"),
        number(semantic, "idle_timeout_ms")
    );
    if let Some(languages) = semantic["languages"].as_array() {
        for language in languages {
            let error = optional_text(language, "last_error")
                .map(|value| format!(" error={}", compact(value)))
                .unwrap_or_default();
            println!(
                "semantic-language: {} health={} available={} startup={} active_roots={} command={}{}",
                text(language, "language"),
                text(language, "health"),
                bool_text(language, "available"),
                text(language, "startup"),
                number(language, "active_roots"),
                text(language, "command"),
                error
            );
        }
    }
    print_string_list("languages", &body["supported_languages"]);
    print_string_list("next", &body["examples"]);
}

fn print_index_summary(index: &Value) {
    println!(
        "index: files={} symbols={} pending={} deleted={} partial={} failed={}",
        number(index, "file_count"),
        number(index, "symbol_count"),
        number(index, "pending_file_count"),
        number(index, "deleted_file_count"),
        number(index, "partial_file_count"),
        number(index, "failed_file_count")
    );
}

fn print_refresh(body: &Value) {
    print_warnings(body);
    println!(
        "refresh: root={} path={} freshness={} confidence={}",
        text(&body["root"], "path"),
        optional_text(body, "path").unwrap_or("."),
        text(body, "freshness"),
        text(body, "confidence")
    );
    let stats = &body["stats"];
    println!(
        "stats: seen={} marked_pending={} deleted={}",
        number(stats, "files_seen"),
        number(stats, "files_marked_pending"),
        number(stats, "files_deleted")
    );
}

fn print_symbol_results(body: &Value) {
    print_envelope(body);
    for result in results(body) {
        let range = format_range(&result["range"]);
        println!(
            "{} {} {} {} {}",
            range,
            text(result, "confidence"),
            text(result, "freshness"),
            text(result, "kind"),
            text(result, "qualified_name")
        );
        if let Some(signature) = optional_text(result, "signature") {
            println!("  signature: {signature}");
        }
        println!("  id: {}", text(result, "id"));
        if !result["body_range"].is_null() {
            println!("  body: {}", format_range(&result["body_range"]));
        }
        println!(
            "  next: sulion-code pack {}",
            pack_target_for_result(result)
        );
    }
}

fn print_reference_results(body: &Value) {
    print_envelope(body);
    for result in results(body) {
        println!(
            "{} {} {} {} {}",
            format_range(&result["range"]),
            text(result, "confidence"),
            text(result, "freshness"),
            text(result, "reference_kind"),
            text(result, "referenced_name")
        );
        if let Some(symbol_id) = optional_text(result, "symbol_id") {
            println!("  symbol: {symbol_id}");
        }
    }
}

fn print_search_results(body: &Value) {
    print_envelope(body);
    for result in results(body) {
        println!(
            "{} {} {} {}",
            format_range(&result["range"]),
            text(result, "confidence"),
            text(result, "freshness"),
            text(result, "kind")
        );
        if let Some(text) = optional_text(result, "text") {
            println!("  match: {}", compact(text));
        }
        if let Some(captures) = result["captures"].as_array() {
            for capture in captures.iter().take(4) {
                println!(
                    "  capture ${}: {} {}",
                    text(capture, "name"),
                    format_range(&capture["range"]),
                    compact(optional_text(capture, "text").unwrap_or(""))
                );
            }
        }
    }
}

fn print_patch(body: &Value) {
    print_warnings(body);
    println!(
        "patch: matches={} applied={} truncated={}",
        number(body, "matches"),
        bool_text(body, "applied"),
        bool_text(body, "truncated")
    );
    if let Some(files) = body["files"].as_array() {
        for file in files {
            println!(
                "file: {} matches={}",
                text(file, "path"),
                number(file, "matches")
            );
        }
    }
    let diff = optional_text(body, "diff").unwrap_or("");
    if diff.is_empty() {
        println!("diff: <empty>");
    } else {
        print!("{diff}");
        if !diff.ends_with('\n') {
            println!();
        }
    }
}

fn print_pack(body: &Value) {
    print_warnings(body);
    let bundle = &body["bundle"];
    let target = &bundle["target"];
    println!(
        "pack: {} {} budget={} confidence={} freshness={}",
        text(target, "kind"),
        format_range(&target["range"]),
        text(body, "budget"),
        text(body, "confidence"),
        text(body, "freshness")
    );
    let primary = &bundle["primary"];
    if primary.is_object() {
        println!(
            "primary: {} {} {}",
            text(primary, "kind"),
            text(primary, "qualified_name"),
            format_range(&primary["range"])
        );
    }
    println!("excerpt: {}", format_range(&bundle["excerpt"]["range"]));
    if let Some(excerpt) = optional_text(&bundle["excerpt"], "text") {
        if !excerpt.is_empty() {
            println!("{excerpt}");
        }
    }
    print_count("containers", &bundle["containers"]);
    print_count("imports", &bundle["imports"]);
    print_count("references", &bundle["references"]);
    print_count("nearby_tests", &bundle["nearby_tests"]);
}

fn print_envelope(body: &Value) {
    print_warnings(body);
    println!(
        "{}: root={} freshness={} confidence={} truncated={}",
        text(body, "command"),
        text(&body["root"], "path"),
        text(body, "freshness"),
        text(body, "confidence"),
        bool_text(body, "truncated")
    );
    if results(body).is_empty() {
        println!("results: none");
    }
}

fn print_warnings(body: &Value) {
    if let Some(warnings) = body["warnings"].as_array() {
        for warning in warnings.iter().filter_map(Value::as_str) {
            eprintln!("warning: {warning}");
        }
    }
}

fn results(body: &Value) -> &[Value] {
    body["results"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn format_range(value: &Value) -> String {
    let path = text(value, "path");
    let start_line = number(value, "start_line");
    let start_col = number(value, "start_col");
    let end_line = number(value, "end_line");
    let end_col = number(value, "end_col");
    format!("{path}:{start_line}:{start_col}-{end_line}:{end_col}")
}

pub(super) fn pack_target_for_result(result: &Value) -> String {
    let id = text(result, "id");
    if id.starts_with("sym_") && !id.contains(':') {
        return id.to_string();
    }
    pack_range_target(&result["range"]).unwrap_or_else(|| id.to_string())
}

fn pack_range_target(value: &Value) -> Option<String> {
    let path = optional_text(value, "path")?;
    let start_line = value.get("start_line").and_then(Value::as_i64)?;
    let end_line = value.get("end_line").and_then(Value::as_i64)?;
    if path.trim().is_empty() || start_line < 1 || end_line < 1 {
        return None;
    }
    Some(format!("{path}:{start_line}-{end_line}"))
}

fn print_string_list(label: &str, value: &Value) {
    let items = value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !items.is_empty() {
        println!("{label}: {items}");
    }
}

fn print_count(label: &str, value: &Value) {
    let count = value.as_array().map(Vec::len).unwrap_or_default();
    println!("{label}: {count}");
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    optional_text(value, key).unwrap_or("-")
}

fn optional_text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or_default()
}

fn bool_text<'a>(value: &'a Value, key: &str) -> &'a str {
    if value.get(key).and_then(Value::as_bool).unwrap_or(false) {
        "true"
    } else {
        "false"
    }
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    );
}
