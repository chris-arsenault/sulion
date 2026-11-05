use std::collections::HashMap;

use crate::ingest::timeline::{
    ProjectionFilters, SpeakerFacet, TimelineAssistantItem, TimelineChunk, TimelineItem,
    TimelineToolPair, TimelineTurn,
};

/// Drops the turn's items and results the filters hide. An item's
/// visibility never changes after it is written, so a delta read filters
/// the same way.
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
    turn.items = std::mem::take(&mut turn.items)
        .into_iter()
        .filter_map(|item| {
            filter_chunk(item.chunk, &pair_by_id, filters).map(|chunk| TimelineItem {
                offset: item.offset,
                chunk,
            })
        })
        .collect();
}

fn filter_chunk(
    mut chunk: TimelineChunk,
    pair_by_id: &HashMap<&str, &TimelineToolPair>,
    filters: &ProjectionFilters,
) -> Option<TimelineChunk> {
    chunk.classify_bookkeeping();
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
        TimelineChunk::Generic {
            label,
            details,
            is_meta,
        } => {
            if !filters.show_bookkeeping && is_meta {
                None
            } else {
                Some(TimelineChunk::Generic {
                    label,
                    details,
                    is_meta,
                })
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
            items: Vec::new(),
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
            is_meta: false,
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
        turn.items = vec![TimelineItem { offset: 0, chunk }];

        let mut hidden = turn.clone();
        apply_projection_filters(&mut hidden, &ProjectionFilters::default());
        assert!(hidden.items.is_empty(), "item visible: {:?}", hidden.items);

        let mut shown = turn;
        let filters = ProjectionFilters {
            show_bookkeeping: true,
            ..Default::default()
        };
        apply_projection_filters(&mut shown, &filters);
        assert_eq!(shown.items.len(), 1);
    }

    #[test]
    fn api_and_collaboration_metadata_hide_but_agent_messages_remain_visible() {
        use crate::ingest::timeline::TimelineGenericDetails;
        let mut turn = turn_with_pair();
        turn.items = [
            "api-request",
            "api-request-blob",
            "inter_agent_communication_metadata",
        ]
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| TimelineItem {
            offset: offset as i64,
            chunk: TimelineChunk::Generic {
                label: kind.into(),
                is_meta: false,
                details: TimelineGenericDetails {
                    event_uuid: None,
                    parent_event_uuid: None,
                    related_tool_use_id: None,
                    subtype: None,
                    speaker: Some("other".into()),
                    content_kind: Some("none".into()),
                    blocks: vec![],
                },
            },
        })
        .collect();
        turn.items.push(TimelineItem {
            offset: 3,
            chunk: TimelineChunk::AgentMessage {
                author: "/root/worker".into(),
                recipient: "/root".into(),
                message_type: None,
                text: "reply".into(),
            },
        });
        let mut hidden = turn.clone();
        apply_projection_filters(&mut hidden, &ProjectionFilters::default());
        assert_eq!(hidden.items.len(), 1);
        assert!(matches!(
            hidden.items[0].chunk,
            TimelineChunk::AgentMessage { .. }
        ));
        apply_projection_filters(
            &mut turn,
            &ProjectionFilters {
                show_bookkeeping: true,
                ..Default::default()
            },
        );
        assert_eq!(turn.items.len(), 4);
    }

    #[test]
    fn generic_visibility_uses_content_and_meta_flags_instead_of_unknown_kind_names() {
        use crate::ingest::canonical::Block;

        for (kind, speaker, blocks, is_meta, expected_meta) in [
            ("session-owner", "other", vec![], false, true),
            ("future-metadata", "other", vec![], false, true),
            (
                "legacy-metadata",
                "other",
                vec![Block::text(0, "state")],
                true,
                true,
            ),
            (
                "future-text",
                "other",
                vec![Block::text(0, "notice")],
                false,
                false,
            ),
            (
                "future-block",
                "other",
                vec![Block::unknown(0, serde_json::json!({"data":"keep"}))],
                false,
                false,
            ),
            ("future-user", "user", vec![], false, false),
        ] {
            // Older stored generic chunks lack is_meta; deserialization must
            // still classify them on both filtered and unfiltered reads.
            let mut body = serde_json::json!({"kind":"generic", "label":kind, "details":{
                "event_uuid":null, "parent_event_uuid":null, "related_tool_use_id":null,
                "subtype":null, "speaker":speaker, "content_kind":"none", "blocks":blocks,
            }});
            if is_meta {
                body["is_meta"] = serde_json::json!(true);
            }
            let chunk = serde_json::from_value(body).unwrap();
            let mut shown = turn_with_pair();
            shown.items.push(TimelineItem { offset: 1, chunk });
            let mut hidden = shown.clone();
            apply_projection_filters(&mut hidden, &ProjectionFilters::default());
            assert_eq!(hidden.items.is_empty(), expected_meta, "{kind}");
            apply_projection_filters(
                &mut shown,
                &ProjectionFilters {
                    show_bookkeeping: true,
                    ..Default::default()
                },
            );
            assert_eq!(
                serde_json::to_value(&shown.items[0]).unwrap()["is_meta"],
                expected_meta,
                "{kind}"
            );
        }
    }

    #[test]
    fn turn_telemetry_system_chunks_hide_with_bookkeeping() {
        let chunk = TimelineChunk::System {
            subtype: Some("stop_hook_summary".to_string()),
            text: String::new(),
            is_meta: false,
        };
        let mut turn = turn_with_pair();
        turn.items = vec![TimelineItem { offset: 0, chunk }];

        let mut hidden = turn.clone();
        apply_projection_filters(&mut hidden, &ProjectionFilters::default());
        assert!(hidden.items.is_empty());

        let mut shown = turn;
        let filters = ProjectionFilters {
            show_bookkeeping: true,
            ..Default::default()
        };
        apply_projection_filters(&mut shown, &filters);
        assert_eq!(shown.items.len(), 1);
    }

    #[test]
    fn no_hidden_speakers_leaves_the_turn_alone() {
        let mut turn = turn_with_pair();
        apply_projection_filters(&mut turn, &ProjectionFilters::default());
        assert_eq!(turn.user_prompt_text.as_deref(), Some("prompt"));
        assert!(turn.tool_pairs[0].result.is_some());
    }
}
