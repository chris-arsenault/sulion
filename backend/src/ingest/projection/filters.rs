use std::collections::HashMap;

use crate::ingest::timeline::{
    ProjectionFilters, SpeakerFacet, TimelineAssistantItem, TimelineChunk, TimelineToolPair,
    TimelineTurn, BOOKKEEPING_KINDS,
};

pub(super) fn apply_projection_filters(turn: &mut TimelineTurn, filters: &ProjectionFilters) {
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
