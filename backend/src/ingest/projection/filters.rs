use std::collections::HashMap;

use crate::ingest::canonical::BlockKind;
use crate::ingest::timeline::{
    is_bookkeeping_system_subtype, is_local_command_text, ProjectionFilters, SpeakerFacet,
    TimelineAssistantItem, TimelineChunk, TimelineToolPair, TimelineTurn, BOOKKEEPING_KINDS,
};

pub(super) fn apply_projection_filters(turn: &mut TimelineTurn, filters: &ProjectionFilters) {
    // The hidden User facet is applied client-side: blanking
    // user_prompt_text here made the detail indistinguishable from a
    // genuine orphan turn ("no user prompt"). The frontend hides the
    // prompt body itself and labels it as filtered.
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
            // Turn telemetry (hook summaries, durations) hides with the
            // meta-flagged system records.
            if !filters.show_bookkeeping
                && (is_meta || is_bookkeeping_system_subtype(subtype.as_deref()))
            {
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
            // Local slash-command user records project as generic chunks;
            // they hide with the rest of the bookkeeping.
            let local_command = details
                .blocks
                .iter()
                .find(|block| block.kind == BlockKind::Text)
                .and_then(|block| block.text.as_deref())
                .is_some_and(is_local_command_text);
            if !filters.show_bookkeeping
                && (BOOKKEEPING_KINDS.contains(&label.as_str()) || local_command)
            {
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
            input_tokens: 0,
            output_tokens: 0,
            markdown: String::new(),
            chunks: Vec::new(),
            pty_session_id: None,
            session_uuid: None,
            session_agent: None,
            session_label: None,
            session_state: None,
        }
    }

    /// The hidden User facet is a client-side render flag — the server
    /// keeps the prompt so a filtered turn stays distinguishable from a
    /// genuine orphan turn.
    #[test]
    fn hiding_the_user_speaker_keeps_the_prompt_in_the_payload() {
        let mut turn = turn_with_pair();
        let mut filters = ProjectionFilters::default();
        filters.hidden_speakers.insert(SpeakerFacet::User);
        apply_projection_filters(&mut turn, &filters);
        assert_eq!(turn.user_prompt_text.as_deref(), Some("prompt"));
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
    fn local_command_generic_chunks_hide_with_bookkeeping() {
        use crate::ingest::canonical::Block;
        use crate::ingest::timeline::TimelineGenericDetails;

        let chunk = TimelineChunk::Generic {
            label: "user".to_string(),
            details: TimelineGenericDetails {
                event_uuid: None,
                parent_event_uuid: None,
                related_tool_use_id: None,
                subtype: None,
                speaker: Some("user".to_string()),
                content_kind: None,
                blocks: vec![Block::text(
                    0,
                    "<local-command-stdout>Set model to X</local-command-stdout>",
                )],
            },
        };
        let mut turn = turn_with_pair();
        turn.chunks = vec![chunk];

        let mut hidden = turn.clone();
        apply_projection_filters(&mut hidden, &ProjectionFilters::default());
        assert!(
            hidden.chunks.is_empty(),
            "chunk visible: {:?}",
            hidden.chunks
        );

        let mut shown = turn;
        let filters = ProjectionFilters {
            show_bookkeeping: true,
            ..Default::default()
        };
        apply_projection_filters(&mut shown, &filters);
        assert_eq!(shown.chunks.len(), 1);
    }

    #[test]
    fn turn_telemetry_system_chunks_hide_with_bookkeeping() {
        let chunk = TimelineChunk::System {
            subtype: Some("stop_hook_summary".to_string()),
            text: String::new(),
            is_meta: false,
        };
        let mut turn = turn_with_pair();
        turn.chunks = vec![chunk];

        let mut hidden = turn.clone();
        apply_projection_filters(&mut hidden, &ProjectionFilters::default());
        assert!(hidden.chunks.is_empty());

        let mut shown = turn;
        let filters = ProjectionFilters {
            show_bookkeeping: true,
            ..Default::default()
        };
        apply_projection_filters(&mut shown, &filters);
        assert_eq!(shown.chunks.len(), 1);
    }

    #[test]
    fn no_hidden_speakers_leaves_the_turn_alone() {
        let mut turn = turn_with_pair();
        apply_projection_filters(&mut turn, &ProjectionFilters::default());
        assert_eq!(turn.user_prompt_text.as_deref(), Some("prompt"));
        assert!(turn.tool_pairs[0].result.is_some());
    }
}
