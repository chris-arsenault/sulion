//! Completed runtime items supplement a model tool call. They are not extra
//! model operations: the reducer folds each into the call with its id, else
//! the single enclosing code execution, and leaves ambiguous evidence as
//! bookkeeping.

use serde_json::{json, Value};

use crate::ingest::canonical::Block;

use super::StoredEvent;

/// The runtime evidence carried by a tool-result block, if it is one.
pub(crate) fn runtime_evidence(block: &Block) -> Option<&Value> {
    block
        .tool_output
        .as_ref()
        .filter(|value| value.get("runtime_item").is_some())
}

/// Evidence that matched no single call stays on its own event as
/// bookkeeping text, with the evidence kept beside it.
pub(crate) fn mark_uncorrelated(event: &mut StoredEvent, evidence: &Value) {
    let item = &evidence["runtime_item"];
    let text = format!(
        "Uncorrelated runtime evidence: {} ({}) — {}",
        item["type"].as_str().unwrap_or("unknown"),
        item["id"].as_str().unwrap_or("unknown id"),
        item["status"].as_str().unwrap_or("completed")
    );
    event.blocks = vec![Block::text(0, text), Block::unknown(1, evidence.clone())];
    event.is_meta = true;
    event.content_kind = Some("text".into());
    event.subtype = Some("runtime_evidence".into());
}

/// Fold one piece of runtime evidence into the call's input: the evidence
/// itself, the command and cwd it ran, and the file edits it made.
pub(crate) fn attach_to_input(input: &mut Option<Value>, evidence: &Value) {
    let item = &evidence["runtime_item"];
    let input = input.get_or_insert_with(|| json!({}));
    if !input.is_object() {
        *input = json!({"original_input": input.clone()});
    }
    push(input, "runtime_items", evidence.clone());
    if item.get("type").and_then(Value::as_str) == Some("CommandExecution") {
        if input.get("command").is_none() {
            if let Some(command) = item.get("command").and_then(Value::as_array) {
                input["command"] = Value::String(
                    command
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
        }
        if input.get("cwd").is_none() {
            input["cwd"] = item["cwd"].clone();
        }
    }
    if let Some(changes) = item.get("changes").and_then(Value::as_object) {
        for (path, change) in changes {
            let edit = json!({"path": change.get("move_path").and_then(Value::as_str).unwrap_or(path),
                "old_path": if change.get("move_path").and_then(Value::as_str).is_some() { Some(path) } else { None },
                "operation": change["type"], "diff": change["unified_diff"], "content": change["content"]});
            let edits = input
                .as_object_mut()
                .unwrap()
                .entry("file_edits")
                .or_insert_with(|| json!([]));
            if let Some(edits) = edits.as_array_mut() {
                if let Some(existing) = edits.iter_mut().find(|e| e["path"] == edit["path"]) {
                    *existing = edit;
                } else {
                    edits.push(edit);
                }
            }
        }
    }
}

/// Fold one piece of runtime evidence into the call's result payload.
pub(crate) fn attach_to_output(output: &mut Option<Value>, evidence: &Value) {
    let payload = output.get_or_insert_with(|| json!({}));
    if !payload.is_object() {
        *payload = json!({"original_output": payload.clone()});
    }
    push(payload, "runtime_items", evidence.clone());
}

pub(crate) fn running_cell(text: Option<&str>) -> Option<&str> {
    // Only the execution envelope signals a running cell. Tool output can quote
    // this phrase while displaying source code or transcript contents.
    text?
        .lines()
        .next()?
        .strip_prefix("Script running with cell ID ")?
        .split_whitespace()
        .next()
}

fn push(object: &mut Value, key: &str, value: Value) {
    let array = object
        .as_object_mut()
        .unwrap()
        .entry(key)
        .or_insert_with(|| json!([]));
    if let Some(array) = array.as_array_mut() {
        array.push(value);
    }
}
