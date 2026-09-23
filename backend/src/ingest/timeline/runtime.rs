use std::{borrow::Cow, collections::HashMap};

use serde_json::{json, Value};

use crate::ingest::canonical::{Block, BlockKind};

use super::StoredEvent;

/// Completed runtime items supplement a model tool call. They are not extra
/// model operations. Use an explicit id first, then a unique enclosing code
/// execution in the same transcript; ambiguous evidence stays visible on its own.
pub(super) fn enrich(events: &[StoredEvent]) -> Cow<'_, [StoredEvent]> {
    if !events.iter().any(|event| {
        event.blocks.iter().any(|block| {
            block
                .tool_output
                .as_ref()
                .is_some_and(|v| v.get("runtime_item").is_some())
        })
    }) {
        return Cow::Borrowed(events);
    }
    let mut output = events.to_vec();
    let mut results = HashMap::new();
    let mut waits = HashMap::new();
    for (i, event) in events.iter().enumerate() {
        for (b, block) in event.blocks.iter().enumerate() {
            if block.kind == BlockKind::ToolResult
                && block
                    .tool_output
                    .as_ref()
                    .is_none_or(|v| v.get("runtime_item").is_none())
            {
                if let Some(id) = block.tool_id.as_deref() {
                    results.insert((event.source_session, id), (i, b));
                }
            }
            if block.kind == BlockKind::ToolUse
                && block
                    .tool_name
                    .as_deref()
                    .and_then(|s| s.rsplit('.').next())
                    == Some("wait")
            {
                if let (Some(cell), Some(id)) = (
                    block
                        .tool_input
                        .as_ref()
                        .and_then(|v| v.get("cell_id"))
                        .and_then(Value::as_str),
                    block.tool_id.as_deref(),
                ) {
                    waits
                        .entry((event.source_session, cell))
                        .or_insert_with(Vec::new)
                        .push(id);
                }
            }
        }
    }
    let mut calls = Vec::new();
    for (i, event) in events.iter().enumerate() {
        for (b, block) in event.blocks.iter().enumerate() {
            if block.kind != BlockKind::ToolUse {
                continue;
            }
            let Some(id) = block.tool_id.as_deref() else {
                continue;
            };
            let mut end = results
                .get(&(event.source_session, id))
                .map(|(i, b)| (*i, &events[*i].blocks[*b]));
            if let Some(cell) = end.and_then(|(_, result)| running_cell(result.text.as_deref())) {
                end = waits
                    .get(&(event.source_session, cell))
                    .into_iter()
                    .flatten()
                    .filter_map(|id| results.get(&(event.source_session, *id)))
                    .map(|(i, b)| (*i, &events[*i].blocks[*b]))
                    .find(|(_, result)| running_cell(result.text.as_deref()).is_none());
            }
            calls.push((i, b, id, end.map(|(i, _)| i)));
        }
    }
    for (runtime_index, event) in events.iter().enumerate() {
        for block in &event.blocks {
            let Some(evidence) = block
                .tool_output
                .as_ref()
                .filter(|v| v.get("runtime_item").is_some())
            else {
                continue;
            };
            let item = &evidence["runtime_item"];
            let started = evidence.get("started_at_ms").and_then(Value::as_i64);
            let mut exact = Vec::new();
            let mut enclosing = Vec::new();
            for &(call_index, block_index, id, finished) in &calls {
                if call_index >= runtime_index {
                    break;
                }
                let call = &events[call_index];
                if call.source_session != event.source_session {
                    continue;
                }
                let tool = &call.blocks[block_index];
                if item.get("id").and_then(Value::as_str) == Some(id) {
                    exact.push((call_index, block_index));
                    continue;
                }
                if tool.tool_name.as_deref().and_then(|s| s.rsplit('.').next()) != Some("exec") {
                    continue;
                }
                let within = started.map_or_else(
                    || finished.is_none_or(|i| i > runtime_index),
                    |start| {
                        start >= call.timestamp.timestamp_millis()
                            && finished
                                .is_none_or(|i| start <= events[i].timestamp.timestamp_millis())
                    },
                );
                if within {
                    enclosing.push((call_index, block_index));
                }
            }
            let candidates = if exact.is_empty() { enclosing } else { exact };
            if candidates.len() != 1 {
                let text = format!(
                    "Uncorrelated runtime evidence: {} ({}) — {}",
                    item["type"].as_str().unwrap_or("unknown"),
                    item["id"].as_str().unwrap_or("unknown id"),
                    item["status"].as_str().unwrap_or("completed")
                );
                let target = &mut output[runtime_index];
                target.blocks = vec![Block::text(0, text), Block::unknown(1, evidence.clone())];
                target.is_meta = false;
                target.content_kind = Some("text".into());
                target.subtype = Some("runtime_evidence".into());
                continue;
            }
            let (call_index, block_index) = candidates[0];
            let tool = &mut output[call_index].blocks[block_index];
            tool.is_error = Some(tool.is_error.unwrap_or(false) || block.is_error.unwrap_or(false));
            let id = tool.tool_id.clone().unwrap();
            let input = tool.tool_input.get_or_insert_with(|| json!({}));
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
                        if let Some(existing) = edits.iter_mut().find(|e| e["path"] == edit["path"])
                        {
                            *existing = edit;
                        } else {
                            edits.push(edit);
                        }
                    }
                }
            }
            if let Some(&(i, b)) = results.get(&(event.source_session, id.as_str())) {
                let result = &mut output[i].blocks[b];
                let payload = result.tool_output.get_or_insert_with(|| json!({}));
                if !payload.is_object() {
                    *payload = json!({"original_output": payload.clone()});
                }
                push(payload, "runtime_items", evidence.clone());
                result.is_error =
                    Some(result.is_error.unwrap_or(false) || block.is_error.unwrap_or(false));
            }
            output[runtime_index].blocks.clear();
        }
    }
    Cow::Owned(output)
}

fn running_cell(text: Option<&str>) -> Option<&str> {
    text?
        .split_once("Script running with cell ID ")?
        .1
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
