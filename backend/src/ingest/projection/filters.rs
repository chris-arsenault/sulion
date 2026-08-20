use std::collections::HashMap;

use crate::ingest::timeline::{
    ProjectionFilters, SpeakerFacet, TimelineAssistantItem, TimelineChunk, TimelineToolPair,
    TimelineTurn, BOOKKEEPING_KINDS,
};

pub(super) fn apply_projection_filters(turn: &mut TimelineTurn, filters: &ProjectionFilters) {
    if filters.hidden_speakers.contains(&SpeakerFacet::User) {
        turn.user_prompt_text = None;
    }
    if filters.hidden_speakers.contains(&SpeakerFacet::ToolResult) {
        for pair in &mut turn.tool_pairs {
            pair.result = None;
        }
    }
    let pair_by_id: HashMap<&str, &TimelineToolPair> = turn
        .tool_pairs
        .iter()
        .map(|pair| (pair.id.as_str(), pair))
        .collect();
    turn.chunks = turn
        .chunks
        .clone()
        .into_iter()
        .filter_map(|chunk| filter_chunk(chunk, &pair_by_id, filters))
        .collect();
}

fn filter_chunk(
    chunk: TimelineChunk,
    pair_by_id: &HashMap<&str, &TimelineToolPair>,
    filters: &ProjectionFilters,
) -> Option<TimelineChunk> {
    match chunk {
        TimelineChunk::Assistant { items, thinking } => {
            if filters.hidden_speakers.contains(&SpeakerFacet::Assistant) {
                return None;
            }
            let items = items
                .into_iter()
                .filter_map(|item| match item {
                    TimelineAssistantItem::Text { .. } => Some(item),
                    TimelineAssistantItem::Tool { pair_id } => pair_by_id
                        .get(pair_id.as_str())
                        .filter(|pair| pair_visible(pair, filters))
                        .map(|_| TimelineAssistantItem::Tool { pair_id }),
                })
                .collect::<Vec<_>>();
            if items.is_empty() && thinking.is_empty() {
                None
            } else {
                Some(TimelineChunk::Assistant { items, thinking })
            }
        }
        TimelineChunk::Tool { pair_id } => {
            if filters.hidden_speakers.contains(&SpeakerFacet::Assistant) {
                return None;
            }
            pair_by_id
                .get(pair_id.as_str())
                .filter(|pair| pair_visible(pair, filters))
                .map(|_| TimelineChunk::Tool { pair_id })
        }
        TimelineChunk::System {
            subtype,
            text,
            is_meta,
        } => {
            if !filters.show_bookkeeping && is_meta {
                None
            } else {
                Some(TimelineChunk::System {
                    subtype,
                    text,
                    is_meta,
                })
            }
        }
        TimelineChunk::Generic { label, details } => {
            if !filters.show_bookkeeping && BOOKKEEPING_KINDS.contains(&label.as_str()) {
                None
            } else {
                Some(TimelineChunk::Generic { label, details })
            }
        }
        other => Some(other),
    }
}

fn pair_visible(pair: &TimelineToolPair, filters: &ProjectionFilters) -> bool {
    pair.category
        .map(|category| !filters.hidden_operation_categories.contains(&category))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::timeline::TimelineToolResult;
    use chrono::Utc;

    fn turn_with_pair() -> TimelineTurn {
        TimelineTurn {
            id: 0,
            turn_key: None,
            preview: "prompt".into(),
            user_prompt_text: Some("prompt".into()),
            start_timestamp: Utc::now(),
            end_timestamp: Utc::now(),
            duration_ms: 0,
            event_count: 2,
            operation_count: 1,
            tool_pairs: vec![TimelineToolPair {
                id: "p1".into(),
                name: "Bash".into(),
                raw_name: None,
                operation_type: Some("bash".into()),
                category: None,
                input: None,
                result: Some(TimelineToolResult {
                    content: Some("output".into()),
                    payload: None,
                    is_error: false,
                }),
                is_error: false,
                is_pending: false,
                file_touches: Vec::new(),
                subagent: None,
            }],
            thinking_count: 0,
            has_errors: false,
            is_sidechain: false,
            markdown: String::new(),
            chunks: Vec::new(),
            pty_session_id: None,
            session_uuid: None,
            session_agent: None,
            session_label: None,
            session_state: None,
        }
    }

    #[test]
    fn hiding_the_user_speaker_blanks_the_prompt() {
        let mut turn = turn_with_pair();
        let mut filters = ProjectionFilters::default();
        filters.hidden_speakers.insert(SpeakerFacet::User);
        apply_projection_filters(&mut turn, &filters);
        assert_eq!(turn.user_prompt_text, None);
        assert!(turn.tool_pairs[0].result.is_some());
    }

    #[test]
    fn hiding_tool_results_strips_pair_results() {
        let mut turn = turn_with_pair();
        let mut filters = ProjectionFilters::default();
        filters.hidden_speakers.insert(SpeakerFacet::ToolResult);
        apply_projection_filters(&mut turn, &filters);
        assert!(turn.tool_pairs[0].result.is_none());
        assert_eq!(turn.user_prompt_text.as_deref(), Some("prompt"));
    }

    #[test]
    fn no_hidden_speakers_leaves_the_turn_alone() {
        let mut turn = turn_with_pair();
        apply_projection_filters(&mut turn, &ProjectionFilters::default());
        assert_eq!(turn.user_prompt_text.as_deref(), Some("prompt"));
        assert!(turn.tool_pairs[0].result.is_some());
    }
}
