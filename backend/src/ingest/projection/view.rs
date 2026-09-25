//! Reads for open views: one turn whole or as what changed after an event
//! offset, a child transcript by reference, and child turns shown beside
//! their parent's.

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use serde_json::Value;
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::timeline::{TimelineAssistantItem, TimelineChunk, TimelineItem};

use super::{
    apply_projection_filters, build_operations_by_turn, load_full_turns, load_projected_turn_row,
    load_projected_turn_rows, load_referenced_turn_ids, subagent_title, ProjectedOperationRow,
    ProjectedTouchRow, ProjectionFilters, TimelineSubagent, TimelineToolPair, TimelineTurn,
    TimelineTurnSummary,
};

/// One turn for an open view.
#[derive(Debug, Clone)]
pub struct TurnView {
    pub turn: TimelineTurn,
    /// The event offset the read reflects; the next `since`.
    pub through: i64,
}

async fn projected_through(pool: &Pool, session_uuid: Uuid) -> anyhow::Result<i64> {
    let through: Option<i64> = sqlx::query_scalar(
        "SELECT projected_through FROM timeline_session_state WHERE session_uuid = $1",
    )
    .bind(session_uuid)
    .fetch_optional(pool)
    .await
    .context("load timeline cursor")?;
    Ok(through.unwrap_or(-1))
}

/// The turn, or with `since` only the items written and operations changed
/// by events after that offset; the header fields always come whole. Calls
/// with a child link come every time, since the child's totals move without
/// the call changing. A delta carries no digest, since composing one needs
/// every item.
pub async fn load_timeline_turn_view(
    pool: &Pool,
    session_uuid: Uuid,
    turn_id: i64,
    filters: &ProjectionFilters,
    since: Option<i64>,
) -> anyhow::Result<Option<TurnView>> {
    // Read first: rows committed later carry a higher offset and are sent
    // again on the next read, never skipped.
    let through = projected_through(pool, session_uuid).await?;
    let referenced = load_referenced_turn_ids(pool, session_uuid, &filters.file_path).await?;
    if referenced
        .as_ref()
        .is_some_and(|ids| !ids.contains(&turn_id))
    {
        return Ok(None);
    }
    let Some(row) =
        load_projected_turn_row(pool, session_uuid, turn_id, filters.errors_only).await?
    else {
        return Ok(None);
    };
    if !filters.show_sidechain && row.is_sidechain_turn {
        return Ok(None);
    }
    let after = since.unwrap_or(i64::MIN);

    let item_rows: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT byte_offset, body FROM timeline_items \
          WHERE session_uuid = $1 AND turn_id = $2 AND byte_offset > $3 \
          ORDER BY byte_offset",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .bind(after)
    .fetch_all(pool)
    .await
    .context("load turn items")?;
    let items: Vec<TimelineItem> = item_rows
        .into_iter()
        .map(|(offset, body)| {
            Ok(TimelineItem {
                offset,
                chunk: serde_json::from_value(body)?,
            })
        })
        .collect::<anyhow::Result<_>>()
        .context("decode turn items")?;

    // Changed operations, linked ones, and any a new item refers to, so the
    // item's visibility can be decided.
    let referenced_pairs: Vec<String> = items
        .iter()
        .flat_map(|item| chunk_pair_ids(&item.chunk))
        .collect();
    let op_rows: Vec<ProjectedOperationRow> = sqlx::query_as(
        "SELECT turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
                operation_category, input, result_content, result_payload, result_is_error, \
                is_error, is_pending \
           FROM timeline_operations o \
          WHERE session_uuid = $1 AND turn_id = $2 \
            AND (changed_at > $3 OR pair_id = ANY($4) \
                 OR EXISTS (SELECT 1 FROM timeline_child_links l \
                             WHERE l.session_uuid = o.session_uuid AND l.pair_id = o.pair_id)) \
          ORDER BY operation_ord",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .bind(after)
    .bind(&referenced_pairs)
    .fetch_all(pool)
    .await
    .context("load turn operations")?;
    let operation_ords: Vec<i32> = op_rows.iter().map(|row| row.operation_ord).collect();
    let touch_rows: Vec<ProjectedTouchRow> = sqlx::query_as(
        "SELECT turn_id, operation_ord, repo_name, repo_rel_path, touch_kind, is_write \
           FROM timeline_file_touches \
          WHERE session_uuid = $1 AND turn_id = $2 AND operation_ord = ANY($3) \
          ORDER BY touch_ord",
    )
    .bind(session_uuid)
    .bind(turn_id)
    .bind(&operation_ords)
    .fetch_all(pool)
    .await
    .context("load turn file touches")?;
    let tool_pairs = build_operations_by_turn(op_rows, touch_rows)?
        .remove(&turn_id)
        .unwrap_or_default();

    let mut turn = super::build_projected_turn(
        row,
        &mut HashMap::from([(turn_id, tool_pairs)]),
        &mut HashMap::from([(turn_id, items)]),
    );
    if since.is_some() {
        turn.markdown.clear();
    }
    {
        let mut pairs: Vec<_> = turn.tool_pairs.iter_mut().collect();
        attach_subagents(pool, session_uuid, &mut pairs).await?;
    }
    apply_projection_filters(&mut turn, filters);
    Ok(Some(TurnView { turn, through }))
}

/// Child transcripts the given calls spawned, by reference. A whole child
/// session reports its own totals; in-file sidechain turns report theirs.
/// A reference whose child has not projected anything yet is left out.
pub(super) async fn attach_subagents(
    pool: &Pool,
    session_uuid: Uuid,
    pairs: &mut [&mut TimelineToolPair],
) -> anyhow::Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let pair_ids: Vec<&str> = pairs.iter().map(|pair| pair.id.as_str()).collect();
    let rows: Vec<(String, Uuid, i64, i64, i64)> = sqlx::query_as(
        "SELECT l.pair_id, l.child_session_uuid, l.child_turn_id, \
                COALESCE(CASE WHEN l.child_turn_id >= 0 THEN ct.event_count::BIGINT \
                              ELSE cs.total_event_count END, 0), \
                COALESCE(CASE WHEN l.child_turn_id >= 0 THEN 1 ELSE cs.turn_count END, 0) \
           FROM timeline_child_links l \
           LEFT JOIN timeline_session_state cs \
                  ON l.child_turn_id < 0 AND cs.session_uuid = l.child_session_uuid \
           LEFT JOIN timeline_turns ct \
                  ON ct.session_uuid = l.child_session_uuid AND ct.turn_id = l.child_turn_id \
          WHERE l.session_uuid = $1 AND l.pair_id = ANY($2) \
          ORDER BY l.pair_id, l.child_turn_id, l.child_session_uuid",
    )
    .bind(session_uuid)
    .bind(&pair_ids)
    .fetch_all(pool)
    .await
    .context("load child references")?;
    let mut by_pair: HashMap<String, TimelineSubagent> = HashMap::new();
    for (pair_id, child, child_turn_id, event_count, turn_count) in rows {
        if event_count <= 0 {
            continue;
        }
        let entry = by_pair.entry(pair_id).or_insert_with(|| TimelineSubagent {
            title: String::new(),
            event_count: 0,
            turn_count: 0,
            session_uuid: Some(child),
            turn_ids: Vec::new(),
        });
        if entry.session_uuid != Some(child) {
            continue;
        }
        if child_turn_id >= 0 {
            entry.turn_ids.push(child_turn_id);
        }
        entry.event_count += event_count as usize;
        entry.turn_count += turn_count as usize;
    }
    for pair in pairs.iter_mut() {
        if let Some(mut subagent) = by_pair.remove(&pair.id) {
            subagent.title = subagent_title(pair);
            pair.subagent = Some(Box::new(subagent));
        }
    }
    Ok(())
}

pub(super) fn chunk_pair_ids(chunk: &TimelineChunk) -> Vec<String> {
    match chunk {
        TimelineChunk::Assistant { items, .. } => items
            .iter()
            .filter_map(|item| match item {
                TimelineAssistantItem::Tool { pair_id } => Some(pair_id.clone()),
                TimelineAssistantItem::Text { .. } => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A child transcript's turns: a whole child session, or the listed turns
/// of one. Sidechain turns are included, since a child is all sidechain.
pub async fn load_session_turns(
    pool: &Pool,
    session_uuid: Uuid,
    turn_ids: Option<&[i64]>,
    filters: &ProjectionFilters,
) -> anyhow::Result<(i64, Vec<TimelineTurn>)> {
    let through = projected_through(pool, session_uuid).await?;
    let wanted: Option<HashSet<i64>> = turn_ids.map(|ids| ids.iter().copied().collect());
    let rows = load_projected_turn_rows(pool, session_uuid, wanted.as_ref(), false).await?;
    let ids: Vec<i64> = rows.iter().map(|row| row.turn_id).collect();
    let mut turns = load_full_turns(pool, session_uuid, rows, &ids).await?;
    for turn in &mut turns {
        apply_projection_filters(turn, filters);
    }
    Ok((through, turns))
}

/// Turn summaries of every session the given one spawned, directly or
/// through its children, for the sidechain view of the parent.
pub(super) async fn load_child_turn_summaries(
    pool: &Pool,
    session_uuid: Uuid,
    filters: &ProjectionFilters,
) -> anyhow::Result<Vec<TimelineTurnSummary>> {
    let children: Vec<(Uuid,)> = sqlx::query_as(
        "WITH RECURSIVE down(session_uuid) AS ( \
             SELECT child_session_uuid FROM timeline_child_links \
              WHERE session_uuid = $1 AND child_turn_id < 0 AND child_session_uuid <> $1 \
             UNION \
             SELECT l.child_session_uuid FROM timeline_child_links l \
               JOIN down ON l.session_uuid = down.session_uuid \
              WHERE l.child_turn_id < 0 AND l.child_session_uuid <> $1 \
         ) \
         SELECT session_uuid FROM down ORDER BY session_uuid",
    )
    .bind(session_uuid)
    .fetch_all(pool)
    .await
    .context("load child sessions")?;
    let mut turns = Vec::new();
    for (child,) in children {
        let mut response = Box::pin(super::load_own_timeline_summary(pool, child, filters)).await?;
        for turn in &mut response.turns {
            turn.turn_key = Some(format!("{child}:{}", turn.id));
            turn.session_uuid = Some(child);
            turn.is_sidechain = true;
        }
        turns.extend(response.turns);
    }
    Ok(turns)
}
