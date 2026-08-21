use serde_json::{Map, Value};

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
    // Codex code-mode `exec` arrives as a JavaScript snippet that wraps
    // one or more `tools.exec_command({"cmd": ...})` calls. Surface the
    // shell commands under the bash-style `command` key so the timeline
    // can summarize the row like the Codex TUI does, and keep the
    // original snippet as `code`.
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

/// Extract the operations a code-mode exec snippet performs: every
/// `"cmd": "…"` shell command (joined `&&`-style under `command`) and
/// every embedded `*** Begin Patch` V4A literal (parsed into the
/// canonical `file_edits` shape). The snippet itself stays under
/// `code`. The values are JSON-compatible string literals inside the
/// JavaScript source, so a quote-aware scan plus serde unescaping
/// recovers them exactly.
pub(crate) fn parse_exec_code(code: &str) -> Value {
    let mut commands: Vec<String> = Vec::new();
    let mut search_from = 0usize;
    while let Some(found) = code[search_from..].find("\"cmd\"") {
        let mut idx = search_from + found + "\"cmd\"".len();
        search_from = idx;
        let bytes = code.as_bytes();
        while idx < bytes.len() && (bytes[idx] as char).is_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() || bytes[idx] != b':' {
            continue;
        }
        idx += 1;
        while idx < bytes.len() && (bytes[idx] as char).is_whitespace() {
            idx += 1;
        }
        let Some((cmd, end)) = js_string_literal_at(code, idx) else {
            continue;
        };
        commands.push(cmd);
        search_from = end;
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
    if !commands.is_empty() {
        out.insert("command".to_string(), Value::String(commands.join(" && ")));
    }
    if !file_edits.is_empty() {
        out.insert("file_edits".to_string(), Value::Array(file_edits));
    }
    out.insert("code".to_string(), Value::String(code.to_string()));
    Value::Object(out)
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
