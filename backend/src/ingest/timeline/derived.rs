use std::collections::HashMap;

use serde_json::Value;

use crate::ingest::canonical::OperationCategory;

use super::file_touches::{extract_file_touches, FileTouchContext};
use super::project::{group_into_turns, project_turn, SUBAGENT_LINK_DEPTH};
use super::{ProjectionFilters, StoredEvent, TimelineChunk, TimelineTurn};

#[derive(Debug, Clone)]
pub struct StoredTurnProjection {
    pub turn_ord: i32,
    pub is_sidechain_turn: bool,
    pub turn: TimelineTurn,
    pub operations: Vec<StoredOperationProjection>,
    pub file_touches: Vec<StoredFileTouchProjection>,
    pub activity_signals: Vec<StoredActivitySignal>,
}

#[derive(Debug, Clone)]
pub struct StoredOperationProjection {
    pub operation_ord: i32,
    pub pair_id: String,
    pub name: String,
    pub raw_name: Option<String>,
    pub operation_type: Option<String>,
    pub operation_category: Option<OperationCategory>,
    pub input: Option<Value>,
    pub result_content: Option<String>,
    pub result_payload: Option<Value>,
    pub result_is_error: bool,
    pub is_error: bool,
    pub is_pending: bool,
    pub file_touches: Vec<super::TimelineFileTouch>,
    pub subagent: Option<super::TimelineSubagent>,
}

#[derive(Debug, Clone)]
pub struct StoredFileTouchProjection {
    pub touch_ord: i32,
    pub operation_ord: Option<i32>,
    pub repo_name: String,
    pub repo_rel_path: String,
    pub touch_kind: String,
    pub is_write: bool,
}

#[derive(Debug, Clone)]
pub struct StoredActivitySignal {
    pub signal_ord: i32,
    pub signal_type: String,
    pub signal_value: Option<String>,
    pub signal_count: i32,
}

pub fn build_session_projection(
    events: &[StoredEvent],
    file_context: Option<&FileTouchContext>,
) -> Vec<StoredTurnProjection> {
    let event_refs: Vec<&StoredEvent> = events.iter().collect();
    group_into_turns(&event_refs)
        .into_iter()
        .enumerate()
        .map(|(turn_ord, seed)| {
            let mut turn = project_turn(
                seed,
                events,
                &ProjectionFilters::default(),
                SUBAGENT_LINK_DEPTH,
            );
            let is_sidechain_turn = turn.is_sidechain;
            let operations = build_operation_projections(&turn, file_context);
            apply_operation_file_touches(&mut turn, &operations);
            let file_touches = build_file_touch_projections(&operations);
            let activity_signals = build_activity_signals(&turn, is_sidechain_turn);
            StoredTurnProjection {
                turn_ord: turn_ord as i32,
                is_sidechain_turn,
                turn,
                operations,
                file_touches,
                activity_signals,
            }
        })
        .collect()
}

fn build_operation_projections(
    turn: &TimelineTurn,
    file_context: Option<&FileTouchContext>,
) -> Vec<StoredOperationProjection> {
    turn.tool_pairs
        .iter()
        .enumerate()
        .map(|(operation_ord, pair)| StoredOperationProjection {
            operation_ord: operation_ord as i32,
            pair_id: pair.id.clone(),
            name: pair.name.clone(),
            raw_name: pair.raw_name.clone(),
            operation_type: pair.operation_type.clone(),
            operation_category: pair.category,
            input: pair.input.clone(),
            result_content: pair
                .result
                .as_ref()
                .and_then(|result| result.content.clone()),
            result_payload: pair
                .result
                .as_ref()
                .and_then(|result| result.payload.clone()),
            result_is_error: pair
                .result
                .as_ref()
                .map(|result| result.is_error)
                .unwrap_or(false),
            is_error: pair.is_error,
            is_pending: pair.is_pending,
            file_touches: extract_file_touches(pair, file_context),
            subagent: pair.subagent.as_deref().cloned(),
        })
        .collect()
}

fn apply_operation_file_touches(turn: &mut TimelineTurn, operations: &[StoredOperationProjection]) {
    for operation in operations {
        let Some(pair) = turn.tool_pairs.get_mut(operation.operation_ord as usize) else {
            continue;
        };
        pair.file_touches = operation.file_touches.clone();
    }
}

fn build_activity_signals(
    turn: &TimelineTurn,
    is_sidechain_turn: bool,
) -> Vec<StoredActivitySignal> {
    let mut out = Vec::new();
    if turn
        .user_prompt_text
        .as_deref()
        .is_some_and(|text| !text.trim().is_empty())
    {
        out.push(new_activity_signal("speaker", Some("user".to_string()), 1));
    }

    let assistant_chunks = turn
        .chunks
        .iter()
        .filter(|chunk| matches!(chunk, TimelineChunk::Assistant { .. }))
        .count() as i32;
    if assistant_chunks > 0 {
        out.push(new_activity_signal(
            "speaker",
            Some("assistant".to_string()),
            assistant_chunks,
        ));
    }

    let tool_result_count = turn
        .tool_pairs
        .iter()
        .filter(|pair| pair.result.is_some())
        .count() as i32;
    if tool_result_count > 0 {
        out.push(new_activity_signal(
            "speaker",
            Some("tool_result".to_string()),
            tool_result_count,
        ));
    }

    let mut category_counts: HashMap<OperationCategory, i32> = HashMap::new();
    for pair in &turn.tool_pairs {
        if let Some(category) = pair.category {
            *category_counts.entry(category).or_insert(0) += 1;
        }
    }
    let mut categories = category_counts.into_iter().collect::<Vec<_>>();
    categories.sort_by_key(|(category, _)| category.as_str().to_string());
    for (category, count) in categories {
        out.push(new_activity_signal(
            "operation_category",
            Some(category.as_str().to_string()),
            count,
        ));
    }

    if turn.thinking_count > 0 {
        out.push(new_activity_signal(
            "thinking",
            None,
            turn.thinking_count as i32,
        ));
    }
    if turn.has_errors {
        let error_count = turn
            .tool_pairs
            .iter()
            .filter(|pair| pair.is_error)
            .count()
            .max(1) as i32;
        out.push(new_activity_signal("error", None, error_count));
    }
    if is_sidechain_turn {
        out.push(new_activity_signal("sidechain", None, 1));
    }

    for (idx, signal) in out.iter_mut().enumerate() {
        signal.signal_ord = idx as i32;
    }
    out
}

fn new_activity_signal(
    signal_type: &str,
    signal_value: Option<String>,
    signal_count: i32,
) -> StoredActivitySignal {
    StoredActivitySignal {
        signal_ord: 0,
        signal_type: signal_type.to_string(),
        signal_value,
        signal_count,
    }
}

fn build_file_touch_projections(
    operations: &[StoredOperationProjection],
) -> Vec<StoredFileTouchProjection> {
    let mut out = Vec::new();
    for operation in operations {
        for touch in &operation.file_touches {
            out.push(StoredFileTouchProjection {
                touch_ord: out.len() as i32,
                operation_ord: Some(operation.operation_ord),
                repo_name: touch.repo.clone(),
                repo_rel_path: touch.path.clone(),
                touch_kind: touch.touch_kind.clone(),
                is_write: touch.is_write,
            });
        }
    }
    out
}
