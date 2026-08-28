use serde_json::{Map, Value};
use tree_sitter::Node;

#[derive(Debug)]
pub(super) struct CodeModeCall {
    pub raw_name: String,
    pub input: Value,
}

/// Parse the real tool calls inside a Codex Code Mode JavaScript wrapper.
/// The transcript keeps the outer `functions.exec` call as raw evidence;
/// canonical blocks use these nested calls as their operation vocabulary.
pub(super) fn parse_tool_calls(code: &str) -> Vec<CodeModeCall> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_javascript::LANGUAGE.into();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(code, None) else {
        return Vec::new();
    };

    let mut calls = Vec::new();
    collect_tool_calls(tree.root_node(), code.as_bytes(), &mut calls);
    calls
}

fn collect_tool_calls(node: Node<'_>, source: &[u8], calls: &mut Vec<CodeModeCall>) {
    if node.kind() == "call_expression" {
        if let Some(call) = tool_call(node, source) {
            calls.push(call);
            return;
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_tool_calls(child, source, calls);
    }
}

fn tool_call(node: Node<'_>, source: &[u8]) -> Option<CodeModeCall> {
    let function = node.child_by_field_name("function")?;
    if function.kind() != "member_expression" {
        return None;
    }
    let object = function.child_by_field_name("object")?;
    if node_text(object, source)? != "tools" {
        return None;
    }
    let property = function.child_by_field_name("property")?;
    let raw_name = node_text(property, source)?.to_string();
    let arguments = node.child_by_field_name("arguments")?;
    let input = arguments
        .named_child(0)
        .and_then(|argument| js_literal(argument, source))
        .unwrap_or_else(|| Value::Object(Map::new()));
    Some(CodeModeCall { raw_name, input })
}

fn js_literal(node: Node<'_>, source: &[u8]) -> Option<Value> {
    match node.kind() {
        "object" => Some(Value::Object(js_object(node, source))),
        "array" => Some(Value::Array(
            named_children(node)
                .filter_map(|child| js_literal(child, source))
                .collect(),
        )),
        "string" => decode_string(node_text(node, source)?).map(Value::String),
        "template_string" => decode_template(node_text(node, source)?).map(Value::String),
        "number" => serde_json::from_str(node_text(node, source)?).ok(),
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        "null" => Some(Value::Null),
        "parenthesized_expression" => node
            .named_child(0)
            .and_then(|child| js_literal(child, source)),
        "unary_expression" => serde_json::from_str(node_text(node, source)?).ok(),
        _ => None,
    }
}

fn js_object(node: Node<'_>, source: &[u8]) -> Map<String, Value> {
    let mut object = Map::new();
    for child in named_children(node) {
        if child.kind() != "pair" {
            continue;
        }
        let Some(key_node) = child.child_by_field_name("key") else {
            continue;
        };
        let Some(value_node) = child.child_by_field_name("value") else {
            continue;
        };
        let Some(key) = property_name(key_node, source) else {
            continue;
        };
        let Some(value) = js_literal(value_node, source) else {
            continue;
        };
        object.insert(key, value);
    }
    object
}

fn property_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source)?;
    if node.kind() == "string" {
        decode_string(text)
    } else {
        Some(text.to_string())
    }
}

fn named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .collect::<Vec<_>>()
        .into_iter()
}

fn node_text<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(source).ok()
}

fn decode_string(raw: &str) -> Option<String> {
    let decoded = if raw.starts_with('"') {
        serde_json::from_str::<String>(raw).ok()?
    } else if raw.starts_with('\'') && raw.ends_with('\'') {
        decode_single_quoted(&raw[1..raw.len().saturating_sub(1)])
    } else {
        return None;
    };
    Some(strip_nul(decoded))
}

fn decode_single_quoted(body: &str) -> String {
    let mut decoded = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(current) = chars.next() {
        if current != '\\' {
            decoded.push(current);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\'') => decoded.push('\''),
            Some('\\') => decoded.push('\\'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

fn decode_template(raw: &str) -> Option<String> {
    if !raw.starts_with('`') || !raw.ends_with('`') || raw.contains("${") {
        return None;
    }
    Some(strip_nul(raw[1..raw.len().saturating_sub(1)].to_string()))
}

fn strip_nul(value: String) -> String {
    if value.contains('\u{0}') {
        value.replace('\u{0}', "")
    } else {
        value
    }
}

pub(super) fn shell_executable(command: &str) -> Option<String> {
    let tokens = first_command_tokens(command);
    let mut index = 0usize;
    while tokens.get(index).is_some_and(|token| is_assignment(token)) {
        index += 1;
    }

    loop {
        let executable = tokens.get(index)?.as_str();
        match executable {
            "env" | "command" | "sudo" => {
                index += 1;
                while tokens
                    .get(index)
                    .is_some_and(|token| token.starts_with('-'))
                {
                    index += 1;
                }
                while tokens.get(index).is_some_and(|token| is_assignment(token)) {
                    index += 1;
                }
            }
            "with-cred" => {
                index += 1;
                if let Some(offset) = tokens[index..].iter().position(|token| token == "--") {
                    index += offset + 1;
                } else if tokens.get(index).is_some() {
                    index += 1;
                }
            }
            "timeout" => {
                index += 1;
                while tokens
                    .get(index)
                    .is_some_and(|token| token.starts_with('-'))
                {
                    index += 1;
                }
                if tokens.get(index).is_some() {
                    index += 1;
                }
            }
            _ => {
                let basename = executable.rsplit('/').next().unwrap_or(executable);
                return Some(basename.to_ascii_lowercase());
            }
        }
    }
}

fn first_command_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for current in command.chars() {
        if escaped {
            token.push(current);
            escaped = false;
            continue;
        }
        if current == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if current == delimiter {
                quote = None;
            } else {
                token.push(current);
            }
            continue;
        }
        if matches!(current, '\'' | '"' | '`') {
            quote = Some(current);
        } else if current.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else if matches!(current, ';' | '|' | '&') {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            break;
        } else {
            token.push(current);
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn is_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}
