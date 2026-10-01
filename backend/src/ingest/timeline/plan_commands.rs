//! Plan invocation metadata derived from recorded shell commands. Shell text
//! is parsed, never evaluated; the CLI remains the argument grammar owner.

use ast_grep_core::tree_sitter::LanguageExt;
use ast_grep_language::SupportLang;
use serde_json::{json, Value};
use tree_sitter::Node;

use crate::ingest::canonical::OperationCategory;

#[derive(Default)]
struct Commands {
    plans: Vec<Value>,
    other: bool,
}

pub(crate) fn project(
    input: &mut Value,
    operation_type: &mut Option<String>,
    category: &mut Option<OperationCategory>,
) -> bool {
    let mut commands = Commands::default();
    inspect_input(input, &mut commands);
    if commands.plans.is_empty() {
        return false;
    }
    if !commands.other {
        *operation_type = Some("sulion_plan".into());
        *category = Some(OperationCategory::Plan);
    }
    input["plan_commands"] = Value::Array(commands.plans);
    true
}

fn inspect_input(input: &Value, commands: &mut Commands) {
    commands.other |= input.get("file_edits").is_some();
    if let Some(operations) = input.get("operations").and_then(Value::as_array) {
        for operation in operations {
            inspect_input(&operation["input"], commands);
        }
        return;
    }
    let Some(command) = input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(Value::as_str)
    else {
        commands.other = true;
        return;
    };
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&SupportLang::Bash.get_ts_language())
        .is_err()
    {
        commands.other = true;
        return;
    }
    if let Some(tree) = parser.parse(command, None) {
        commands.other |= tree.root_node().has_error();
        visit(tree.root_node(), command, commands);
    }
}

fn visit(node: Node<'_>, source: &str, commands: &mut Commands) {
    match node.kind() {
        "command" => {
            let tokens = command_tokens(node, source);
            match plan_arguments(&tokens) {
                Some(args) => commands.plans.push(metadata(args)),
                None => commands.other |= tokens.first().and_then(Option::as_deref) != Some("cd"),
            }
            return;
        }
        // Quoted examples, heredoc bodies and function declarations are not
        // invocations of the commands they contain.
        "function_definition" | "heredoc_body" | "string" | "raw_string" | "comment" => return,
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, source, commands);
    }
}

fn command_tokens(node: Node<'_>, source: &str) -> Vec<Option<String>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind(),
                "variable_assignment" | "file_redirect" | "heredoc_redirect"
            )
        })
        .map(|child| literal(child, source))
        .collect()
}

fn literal(node: Node<'_>, source: &str) -> Option<String> {
    let text = node.utf8_text(source.as_bytes()).ok()?;
    match node.kind() {
        "raw_string" => Some(text.strip_prefix('\'')?.strip_suffix('\'')?.to_string()),
        "command_name" | "concatenation" => {
            let mut cursor = node.walk();
            node.named_children(&mut cursor)
                .map(|child| literal(child, source))
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.concat())
        }
        "word" | "number" | "string_content" => unescape(text, false),
        "string" => {
            let mut cursor = node.walk();
            if node
                .named_children(&mut cursor)
                .any(|child| child.kind() != "string_content")
            {
                return None;
            }
            unescape(text.strip_prefix('"')?.strip_suffix('"')?, true)
        }
        _ => None,
    }
}

fn unescape(text: &str, quoted: bool) -> Option<String> {
    let mut output = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        let next = chars.next()?;
        if next == '\n' {
            continue;
        }
        if quoted && !matches!(next, '$' | '`' | '"' | '\\') {
            output.push('\\');
        }
        output.push(next);
    }
    Some(output)
}

fn plan_arguments(tokens: &[Option<String>]) -> Option<&[Option<String>]> {
    let mut index = 0;
    loop {
        let token = tokens.get(index)?.as_deref()?;
        let program = token.rsplit('/').next()?;
        match program {
            "env" | "command" | "sudo" => {
                index += 1;
                while tokens
                    .get(index)
                    .and_then(Option::as_deref)
                    .is_some_and(|word| word == "--" || word == "-n" || word.contains('='))
                {
                    index += 1;
                }
            }
            "with-cred" => {
                index += 1;
                if tokens.get(index)?.as_deref()? != "--" {
                    return None;
                }
                index += 1;
            }
            "timeout" => {
                index += 1;
                if tokens.get(index)?.as_deref()? == "--" {
                    index += 1;
                }
                tokens.get(index)?.as_deref()?;
                index += 1;
            }
            "sulion" if tokens.get(index + 1)?.as_deref()? == "plan" => {
                return Some(&tokens[index + 2..])
            }
            _ => return None,
        }
    }
}

fn metadata(tokens: &[Option<String>]) -> Value {
    let mut tokens = tokens.to_vec();
    if let Some(index) = tokens
        .iter()
        .position(|token| token.as_deref() == Some("--json"))
    {
        tokens.remove(index);
    }
    let command = tokens
        .first()
        .map_or("help", |token| token.as_deref().unwrap_or("unknown"));
    let action = match command {
        "phase" | "step" => tokens
            .get(1)
            .and_then(Option::as_deref)
            .map_or_else(|| "phase".into(), |action| format!("phase {action}")),
        "-h" | "--help" => "help".into(),
        _ => command.to_owned(),
    };
    let mut output = json!({"action": action});
    if tokens.iter().any(Option::is_none) || tokens.is_empty() {
        return output;
    }
    let mut args: Vec<String> = tokens.iter().filter_map(Clone::clone).collect();
    args.remove(0);
    let Ok(request) = crate::plan_cli::parse_plan_request(command, &mut args) else {
        return output;
    };
    let Ok(request) = serde_json::to_value(request) else {
        return output;
    };
    let fields = request
        .get("input")
        .or_else(|| request.get("phase"))
        .unwrap_or(&request);
    for key in [
        "title",
        "summary",
        "description",
        "status",
        "note",
        "outcome",
        "principles",
        "assumptions",
        "size",
        "phases",
    ] {
        if let Some(value) = fields.get(key).filter(|value| !value.is_null()) {
            output[key] = value.clone();
        }
    }
    if let Some(note) = fields.get("status_note").filter(|value| !value.is_null()) {
        output["note"] = note.clone();
    }
    for (source, target) in [
        ("plan_id", "plan_id"),
        ("phase_reference", "phase"),
        ("parent_phase_refs", "from"),
    ] {
        if let Some(value) = request
            .get(source)
            .or_else(|| fields.get(source))
            .filter(|value| !value.is_null())
        {
            output[target] = value.clone();
        }
    }
    if let Some(phases) = output.get("phases").and_then(Value::as_array) {
        output["phase_count"] = json!(phases.len());
    }
    output
}

#[cfg(test)]
mod tests;
