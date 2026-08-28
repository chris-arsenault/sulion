use serde_json::{Map, Value};

use super::code_mode::{parse_tool_calls, shell_executable};

pub fn canonicalize_tool_name(raw: &str) -> String {
    match raw {
        // Claude Code builtins
        "Read" | "read_file" => "read",
        "Write" | "write_file" => "write",
        "Edit" | "apply_diff" => "edit",
        "MultiEdit" => "multi_edit",
        "Bash" | "shell" | "execute_shell" => "bash",
        "Grep" | "grep_search" => "grep",
        "Glob" | "glob_files" => "glob",
        // `Agent` is the background-agent successor to Task: same
        // description/subagent_type/prompt input, same delegation
        // semantics, so the whole task pipeline (category, badges,
        // subagent projection) applies.
        "Task" | "Agent" | "spawn_agent" => "task",
        "TodoWrite" | "todo_update" => "todo_write",
        "WebFetch" | "fetch_url" => "web_fetch",
        "WebSearch" | "web_search_query" => "web_search",
        "NotebookEdit" => "notebook_edit",
        "BashOutput" => "bash_output",
        "KillShell" | "killShell" => "kill_shell",
        "ExitPlanMode" | "exitPlanMode" => "exit_plan_mode",
        other => {
            // Fallback: lowercase + snake_case. Keeps unknown agents
            // pointing at something consistent.
            return to_snake_case(other);
        }
    }
    .to_string()
}

pub(crate) fn canonicalize_tool_use(raw_name: &str, input: Value) -> (String, Value) {
    let canonical_name = canonicalize_tool_name(raw_name);
    let mut input = canonicalize_tool_input(&canonical_name, input);
    let effective_name = if canonical_name == "exec" {
        input
            .as_object_mut()
            .and_then(|object| object.remove("operation_name"))
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or(canonical_name)
    } else {
        canonical_name
    };
    (effective_name, input)
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_lower = false;
        } else if c == '-' || c == ' ' {
            out.push('_');
            prev_lower = false;
        } else {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

pub(crate) fn canonicalize_tool_input(canonical_name: &str, input: Value) -> Value {
    // apply_patch arrives as a raw V4-patch string, not an object.
    // Parse it into the agent-agnostic `file_edits` shape before the
    // object-only early-return below.
    if canonical_name == "apply_patch" {
        if let Value::String(raw) = &input {
            return parse_apply_patch(raw);
        }
    }
    // Codex Code Mode `exec` is a JavaScript transport around one or
    // more `tools.*` calls. Canonicalize those nested operations and
    // keep the original snippet as `code`.
    if canonical_name == "exec" {
        if let Value::String(raw) = &input {
            return parse_exec_code(raw);
        }
    }
    let Value::Object(obj) = input else {
        return input;
    };
    let mut out = Map::new();
    match canonical_name {
        "read" => {
            copy_first(&obj, &mut out, "path", &["path", "file_path"]);
            copy_key(&obj, &mut out, "offset");
            copy_key(&obj, &mut out, "limit");
        }
        "write" => {
            copy_first(&obj, &mut out, "path", &["path", "file_path"]);
            copy_key(&obj, &mut out, "content");
        }
        "edit" => {
            // Canonicalise to the tool-agnostic `file_edits` shape —
            // same structure the frontend's FileEditRenderer consumes
            // for multi_edit and codex apply_patch. `in_out` carries
            // the authoritative before/after strings (no reconstruction).
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .cloned()
                .unwrap_or(Value::Null);
            let old_text = obj
                .get("old_text")
                .or_else(|| obj.get("old_string"))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let new_text = obj
                .get("new_text")
                .or_else(|| obj.get("new_string"))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let replace_all = obj.get("replace_all").cloned();
            let mut entry = Map::new();
            entry.insert("path".to_string(), path);
            entry.insert("operation".to_string(), Value::String("update".into()));
            let mut in_out = Map::new();
            in_out.insert("old_text".to_string(), old_text);
            in_out.insert("new_text".to_string(), new_text);
            entry.insert("in_out".to_string(), Value::Object(in_out));
            if let Some(value) = replace_all {
                entry.insert("replace_all".to_string(), value);
            }
            out.insert(
                "file_edits".to_string(),
                Value::Array(vec![Value::Object(entry)]),
            );
        }
        "multi_edit" => {
            // N edits against one path → N file_edits entries sharing
            // that path. The renderer groups consecutive same-path
            // entries; no backend-side consolidation needed.
            let path = obj
                .get("path")
                .or_else(|| obj.get("file_path"))
                .cloned()
                .unwrap_or(Value::Null);
            let edits: Vec<Value> = match obj.get("edits") {
                Some(Value::Array(edits)) => edits
                    .iter()
                    .map(|edit| multi_edit_to_file_edit(&path, edit))
                    .collect(),
                _ => Vec::new(),
            };
            out.insert("file_edits".to_string(), Value::Array(edits));
        }
        "bash" => {
            copy_key(&obj, &mut out, "command");
            copy_key(&obj, &mut out, "description");
        }
        "grep" => {
            copy_key(&obj, &mut out, "pattern");
            copy_first(&obj, &mut out, "path", &["path", "glob"]);
            copy_first(&obj, &mut out, "mode", &["mode", "output_mode"]);
        }
        "glob" => {
            copy_key(&obj, &mut out, "pattern");
            copy_key(&obj, &mut out, "path");
        }
        "task" => {
            copy_first(&obj, &mut out, "agent", &["agent", "subagent_type"]);
            copy_key(&obj, &mut out, "description");
            copy_key(&obj, &mut out, "prompt");
        }
        "todo_write" => {
            copy_key(&obj, &mut out, "todos");
        }
        "web_fetch" => {
            copy_key(&obj, &mut out, "url");
            copy_key(&obj, &mut out, "prompt");
        }
        "web_search" => {
            copy_key(&obj, &mut out, "query");
            copy_key(&obj, &mut out, "prompt");
        }
        _ => {
            return Value::Object(obj);
        }
    }
    Value::Object(out)
}

pub(crate) fn canonicalize_tool_result_payload(payload: Value) -> Value {
    normalize_result_value(payload)
}

/// Replace the outer Code Mode `exec` transport with the operations in
/// its JavaScript body. A single call takes the same canonical input
/// shape as its direct counterpart. Multiple calls retain an ordered
/// `operations` list, while aggregate commands and file edits stay at
/// the top level for existing renderers and file-touch extraction.
pub(crate) fn parse_exec_code(code: &str) -> Value {
    let calls = parse_tool_calls(code);
    let mut commands = Vec::new();
    let mut operation_names = Vec::new();
    let mut operations = Vec::new();
    let mut single_input = None;
    for call in calls {
        let tool_name = code_mode_tool_name(&call.raw_name);
        let input = canonicalize_tool_input(&tool_name, call.input);
        let operation_name = code_mode_operation_name(&call.raw_name, &tool_name, &input);
        if tool_name == "exec_command" {
            if let Some(command) = input
                .as_object()
                .and_then(|object| object.get("command").or_else(|| object.get("cmd")))
                .and_then(Value::as_str)
            {
                commands.push(command.to_string());
            }
        }
        operation_names.push(operation_name.clone());

        let mut operation = Map::new();
        operation.insert("name".into(), Value::String(operation_name));
        operation.insert("raw_name".into(), Value::String(call.raw_name));
        operation.insert("input".into(), input.clone());
        operations.push(Value::Object(operation));
        if operations.len() == 1 {
            single_input = Some(input);
        } else {
            single_input = None;
        }
    }

    let mut file_edits: Vec<Value> = Vec::new();
    let mut search_from = 0usize;
    while let Some(found) = code[search_from..].find("\"*** Begin Patch") {
        let literal_start = search_from + found;
        let Some((patch, end)) = js_string_literal_at(code, literal_start) else {
            search_from = literal_start + 1;
            continue;
        };
        if let Some(Value::Array(entries)) = parse_apply_patch(&patch).get("file_edits") {
            file_edits.extend(entries.iter().cloned());
        }
        search_from = end;
    }

    let mut out = Map::new();
    let has_single_input = single_input.is_some();
    if let Some(Value::Object(input)) = single_input {
        out.extend(input);
    } else if !operations.is_empty() {
        out.insert("operations".to_string(), Value::Array(operations));
    }
    if commands.len() > 1 || (!has_single_input && !commands.is_empty()) {
        out.insert("command".to_string(), Value::String(commands.join(" && ")));
    }
    if !file_edits.is_empty() {
        out.insert("file_edits".to_string(), Value::Array(file_edits));
    }
    if !operation_names.is_empty() {
        let first = &operation_names[0];
        let operation_name = if !out.contains_key("file_edits")
            && operation_names.iter().all(|name| name == first)
        {
            first.clone()
        } else if out.contains_key("file_edits") {
            "apply_patch".to_string()
        } else {
            "parallel".to_string()
        };
        out.insert("operation_name".to_string(), Value::String(operation_name));
    }
    out.insert("code".to_string(), Value::String(code.to_string()));
    Value::Object(out)
}

fn code_mode_tool_name(raw_name: &str) -> String {
    let unnamespaced = raw_name.rsplit("__").next().unwrap_or(raw_name);
    canonicalize_tool_name(unnamespaced)
}

fn code_mode_operation_name(raw_name: &str, tool_name: &str, input: &Value) -> String {
    if tool_name == "exec_command" {
        return input
            .as_object()
            .and_then(|object| object.get("cmd").or_else(|| object.get("command")))
            .and_then(Value::as_str)
            .and_then(shell_executable)
            .unwrap_or_else(|| tool_name.to_string());
    }
    if raw_name == "web__run" {
        let actions: Vec<&str> = [
            "search_query",
            "image_query",
            "open",
            "click",
            "find",
            "screenshot",
            "weather",
            "finance",
            "sports",
            "time",
        ]
        .into_iter()
        .filter(|key| input.get(*key).is_some_and(|value| !value.is_null()))
        .collect();
        return match actions.as_slice() {
            [action] => (*action).to_string(),
            [] => tool_name.to_string(),
            _ => "parallel".to_string(),
        };
    }
    tool_name.to_string()
}

/// Decode the double-quoted string literal starting at byte `start`.
/// Returns the decoded value and the byte index just past the closing
/// quote. Quotes and backslashes are ASCII, so a byte walk is safe in
/// UTF-8 source.
fn js_string_literal_at(code: &str, start: usize) -> Option<(String, usize)> {
    let bytes = code.as_bytes();
    if start >= bytes.len() || bytes[start] != b'"' {
        return None;
    }
    let mut idx = start + 1;
    let mut escaped = false;
    while idx < bytes.len() {
        match (escaped, bytes[idx]) {
            (true, _) => escaped = false,
            (false, b'\\') => escaped = true,
            (false, b'"') => {
                let decoded = serde_json::from_str::<String>(&code[start..=idx]).ok()?;
                // A decoded NUL (an escaped u0000 in the JS source)
                // cannot be stored: Postgres rejects it in both TEXT
                // and JSONB values.
                let decoded = if decoded.contains('\u{0}') {
                    decoded.replace('\u{0}', "")
                } else {
                    decoded
                };
                return Some((decoded, idx + 1));
            }
            _ => {}
        }
        idx += 1;
    }
    None
}

fn multi_edit_to_file_edit(path: &Value, edit: &Value) -> Value {
    let obj = match edit {
        Value::Object(obj) => obj,
        _ => return Value::Null,
    };
    let old_text = obj
        .get("old_text")
        .or_else(|| obj.get("old_string"))
        .cloned()
        .unwrap_or(Value::String(String::new()));
    let new_text = obj
        .get("new_text")
        .or_else(|| obj.get("new_string"))
        .cloned()
        .unwrap_or(Value::String(String::new()));
    let replace_all = obj.get("replace_all").cloned();
    let mut entry = Map::new();
    entry.insert("path".to_string(), path.clone());
    entry.insert("operation".to_string(), Value::String("update".into()));
    let mut in_out = Map::new();
    in_out.insert("old_text".to_string(), old_text);
    in_out.insert("new_text".to_string(), new_text);
    entry.insert("in_out".to_string(), Value::Object(in_out));
    if let Some(value) = replace_all {
        entry.insert("replace_all".to_string(), value);
    }
    Value::Object(entry)
}

/// Structural parse of codex's V4 apply_patch envelope into the
/// tool-agnostic `file_edits` shape. Directive headers give the path
/// and operation; the raw lines between directives are the `diff`
/// payload, passed through verbatim for the frontend's unified-diff
/// renderer to consume. No reconstruction, no per-hunk splitting.
pub(crate) fn parse_apply_patch(raw: &str) -> Value {
    let mut entries: Vec<Value> = Vec::new();
    let mut current: Option<FileEditBuf> = None;

    for line in raw.lines() {
        if let Some(header) = line.strip_prefix("*** ") {
            if let Some(buf) = current.take() {
                entries.push(buf.into_value());
            }
            if header == "Begin Patch" || header == "End Patch" {
                continue;
            }
            if let Some(path) = header.strip_prefix("Update File: ") {
                current = Some(FileEditBuf::new(path.trim(), "update", None));
            } else if let Some(path) = header.strip_prefix("Add File: ") {
                current = Some(FileEditBuf::new(path.trim(), "add", None));
            } else if let Some(path) = header.strip_prefix("Delete File: ") {
                current = Some(FileEditBuf::new(path.trim(), "delete", None));
            } else if let Some(rest) = header.strip_prefix("Move File: ") {
                let (from, to) = split_move(rest).unwrap_or((rest.trim(), rest.trim()));
                current = Some(FileEditBuf::new(to, "move", Some(from.to_string())));
            }
            continue;
        }
        if let Some(buf) = current.as_mut() {
            buf.diff.push_str(line);
            buf.diff.push('\n');
        }
    }
    if let Some(buf) = current.take() {
        entries.push(buf.into_value());
    }

    let mut out = Map::new();
    out.insert("file_edits".to_string(), Value::Array(entries));
    Value::Object(out)
}

fn split_move(rest: &str) -> Option<(&str, &str)> {
    let idx = rest.find(" to ")?;
    let from = rest[..idx].trim();
    let to = rest[idx + 4..].trim();
    if from.is_empty() || to.is_empty() {
        return None;
    }
    Some((from, to))
}

struct FileEditBuf {
    path: String,
    operation: &'static str,
    old_path: Option<String>,
    diff: String,
}

impl FileEditBuf {
    fn new(path: &str, operation: &'static str, old_path: Option<String>) -> Self {
        Self {
            path: path.to_string(),
            operation,
            old_path,
            diff: String::new(),
        }
    }

    fn into_value(self) -> Value {
        let mut obj = Map::new();
        obj.insert("path".to_string(), Value::String(self.path));
        obj.insert(
            "operation".to_string(),
            Value::String(self.operation.to_string()),
        );
        if let Some(old_path) = self.old_path {
            obj.insert("old_path".to_string(), Value::String(old_path));
        }
        obj.insert(
            "diff".to_string(),
            Value::String(self.diff.trim_end_matches('\n').to_string()),
        );
        Value::Object(obj)
    }
}

fn normalize_result_value(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(normalize_result_value)
                .collect::<Vec<_>>(),
        ),
        Value::Object(obj) => {
            let mut out = Map::with_capacity(obj.len());
            for (key, value) in obj {
                out.insert(normalize_result_key(&key), normalize_result_value(value));
            }
            Value::Object(out)
        }
        other => other,
    }
}

fn normalize_result_key(raw: &str) -> String {
    match raw {
        "filePath" | "file_path" => "path".to_string(),
        "oldString" | "old_string" => "old_text".to_string(),
        "newString" | "new_string" => "new_text".to_string(),
        "replaceAll" | "replace_all" => "replace_all".to_string(),
        "originalFile" | "original_file" => "original_file".to_string(),
        "structuredPatch" | "structured_patch" => "structured_patch".to_string(),
        "userModified" | "user_modified" => "user_modified".to_string(),
        other => to_snake_case(other),
    }
}

fn copy_first(
    src: &Map<String, Value>,
    dst: &mut Map<String, Value>,
    target: &str,
    candidates: &[&str],
) {
    for key in candidates {
        if let Some(value) = src.get(*key) {
            dst.insert(target.to_string(), value.clone());
            return;
        }
    }
}

fn copy_key(src: &Map<String, Value>, dst: &mut Map<String, Value>, key: &str) {
    if let Some(value) = src.get(key) {
        dst.insert(key.to_string(), value.clone());
    }
}
