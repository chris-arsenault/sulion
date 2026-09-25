//! Finite, resumable reads of the existing projection. No transaction or
//! connection is retained while the HTTP consumer waits for its next batch.
use std::collections::HashMap;

use anyhow::{ensure, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db::Pool;
use crate::ingest::timeline::{TimelineItem, TimelineTurn};

use super::{build_operations_by_turn, ProjectedOperationRow, ProjectedTouchRow};

pub const BATCH_SIZE: i64 = 64;

pub async fn summaries(
    pool: &Pool,
    session: Uuid,
    filters: &super::ProjectionFilters,
) -> anyhow::Result<super::TimelineSummaryResponse> {
    super::load_own_timeline_summary(pool, session, filters).await
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cursor {
    pub turn_id: i64,
    pub generation: String,
    pub since: i64,
    pub through: Option<i64>,
    pub item_after: i64,
    pub operation_after: i32,
    pub items_done: bool,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Header {
        turn: Box<TimelineTurn>,
        archived_at: Option<chrono::DateTime<chrono::Utc>>,
        cursor: Cursor,
        reset: bool,
    },
    Batch {
        items: Vec<TimelineItem>,
        operations: Vec<Value>,
        cursor: Cursor,
    },
    Complete {
        cursor: Cursor,
    },
    Reset,
    Error {
        message: String,
    },
}

pub async fn position(pool: &Pool, session: Uuid) -> anyhow::Result<(String, i64)> {
    // Purging changes the available data without advancing transcript offsets.
    let row: (String, i64) = sqlx::query_as(
        "SELECT s.generation::text || ':' || COALESCE(cs.purged_at::text, ''), s.projected_through \
         FROM timeline_session_state s JOIN claude_sessions cs USING (session_uuid) \
         WHERE s.session_uuid = $1",
    ).bind(session).fetch_one(pool).await?;
    Ok(row)
}

pub async fn begin(
    pool: &Pool,
    session: Uuid,
    turn_id: i64,
    previous: Option<Cursor>,
) -> anyhow::Result<Record> {
    let (generation, through) = position(pool, session).await?;
    let reset = previous
        .as_ref()
        .is_none_or(|c| c.turn_id != turn_id || c.generation != generation || c.since > through);
    let mut cursor = previous.filter(|_| !reset).unwrap_or(Cursor {
        turn_id,
        generation,
        since: -1,
        through: None,
        item_after: -1,
        operation_after: -1,
        items_done: false,
    });
    ensure!(
        cursor.since >= -1 && cursor.item_after >= -1 && cursor.operation_after >= -1,
        "invalid turn cursor"
    );
    ensure!(
        cursor
            .through
            .is_none_or(|offset| offset >= cursor.since && offset <= through),
        "invalid turn checkpoint"
    );
    cursor.through.get_or_insert(through);
    let row = super::load_projected_turn_row(pool, session, turn_id, false)
        .await?
        .context("turn not found")?;
    let mut turn = super::build_projected_turn(row, &mut HashMap::new(), &mut HashMap::new());
    let meta = super::load_timeline_session_meta(pool, session).await?;
    if meta.archived_at.is_none() {
        turn.markdown.clear();
    }
    super::annotate_timeline_turns(std::slice::from_mut(&mut turn), &meta);
    ensure!(
        position(pool, session).await?.0 == cursor.generation,
        "projection changed during read"
    );
    Ok(Record::Header {
        turn: Box::new(turn),
        archived_at: meta.archived_at,
        cursor,
        reset,
    })
}

#[derive(sqlx::FromRow)]
struct Operation {
    #[sqlx(flatten)]
    row: ProjectedOperationRow,
    changed_at: i64,
}

// Only the fields needed by tool headers and subagent titles cross the wire.
// Full command/source/output bodies are served by operation_body on demand.
const COMPACT_INPUT: &str = "jsonb_strip_nulls(jsonb_build_object(\
    'path', left(input->>'path', 240), \
    'file_edits', CASE WHEN input #>> '{file_edits,0,path}' IS NOT NULL THEN \
        jsonb_build_array(jsonb_build_object('path', left(input #>> '{file_edits,0,path}', 240))) END, \
    'file_path', left(input->>'file_path', 240), \
    'command', left(input->>'command', 240), 'cmd', left(input->>'cmd', 240), \
    'description', left(input->>'description', 240), 'agent', left(input->>'agent', 240), \
    'pattern', left(input->>'pattern', 240), 'url', left(input->>'url', 240), \
    'query', left(input->>'query', 240)))";

async fn load_operations(
    pool: &Pool,
    session: Uuid,
    turn: i64,
    cursor: &Cursor,
    references: Option<&[String]>,
) -> anyhow::Result<(Vec<Value>, i32)> {
    let sql = format!(
        "SELECT turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
         operation_category, {COMPACT_INPUT} AS input, NULL::text AS result_content, \
         NULL::jsonb AS result_payload, result_is_error, is_error, is_pending, changed_at \
         FROM timeline_operations o WHERE session_uuid = $1 AND turn_id = $2 \
         AND (($3 AND pair_id = ANY($4)) OR (NOT $3 AND operation_ord > $5 \
           AND (changed_at > $6 OR EXISTS (SELECT 1 FROM timeline_child_links l \
                WHERE l.session_uuid = o.session_uuid AND l.pair_id = o.pair_id)))) \
         ORDER BY operation_ord LIMIT $7"
    );
    let rows: Vec<Operation> = sqlx::query_as(&sql)
        .bind(session)
        .bind(turn)
        .bind(references.is_some())
        .bind(references.unwrap_or(&[]))
        .bind(cursor.operation_after)
        .bind(cursor.since)
        // One event can reference more calls than the ordinary page size.
        .bind(references.map_or(BATCH_SIZE, |ids| ids.len() as i64))
        .fetch_all(pool)
        .await?;
    let last = rows
        .last()
        .map_or(cursor.operation_after, |r| r.row.operation_ord);
    let versions: HashMap<_, _> = rows
        .iter()
        .map(|r| (r.row.pair_id.clone(), r.changed_at))
        .collect();
    let ords: Vec<_> = rows.iter().map(|r| r.row.operation_ord).collect();
    let touches: Vec<ProjectedTouchRow> = sqlx::query_as(
        "SELECT turn_id, operation_ord, repo_name, repo_rel_path, touch_kind, is_write \
         FROM timeline_file_touches WHERE session_uuid = $1 AND turn_id = $2 \
         AND operation_ord = ANY($3) ORDER BY touch_ord",
    )
    .bind(session)
    .bind(turn)
    .bind(ords)
    .fetch_all(pool)
    .await?;
    let mut pairs = build_operations_by_turn(rows.into_iter().map(|r| r.row).collect(), touches)?
        .remove(&turn)
        .unwrap_or_default();
    super::view::attach_subagents(pool, session, &mut pairs.iter_mut().collect::<Vec<_>>()).await?;
    let values = pairs
        .into_iter()
        .map(|pair| {
            let version = versions[&pair.id];
            let mut value = serde_json::to_value(pair)?;
            value["body_version"] = json!(version);
            value["body_loaded"] = json!(false);
            Ok(value)
        })
        .collect::<anyhow::Result<_>>()?;
    Ok((values, last))
}

pub async fn next(
    pool: &Pool,
    session: Uuid,
    turn: i64,
    mut cursor: Cursor,
) -> anyhow::Result<Record> {
    if cursor.turn_id != turn || position(pool, session).await?.0 != cursor.generation {
        return Ok(Record::Reset);
    }
    let mut items = Vec::new();
    let operations;
    if !cursor.items_done {
        let rows: Vec<(i64, Value)> = sqlx::query_as(
            "SELECT byte_offset, body FROM timeline_items WHERE session_uuid = $1 AND turn_id = $2 \
             AND byte_offset > $3 AND byte_offset <= $4 ORDER BY byte_offset LIMIT $5"
        ).bind(session).bind(turn).bind(cursor.item_after).bind(cursor.through)
            .bind(BATCH_SIZE).fetch_all(pool).await?;
        cursor.items_done = rows.len() < BATCH_SIZE as usize;
        for (offset, body) in rows {
            items.push(TimelineItem {
                offset,
                chunk: serde_json::from_value(body)?,
            });
            cursor.item_after = offset;
        }
        let references: Vec<_> = items
            .iter()
            .flat_map(|i| super::view::chunk_pair_ids(&i.chunk))
            .collect();
        operations = if references.is_empty() {
            Vec::new()
        } else {
            load_operations(pool, session, turn, &cursor, Some(&references))
                .await?
                .0
        };
    } else {
        let (values, last) = load_operations(pool, session, turn, &cursor, None).await?;
        cursor.operation_after = last;
        if values.is_empty() {
            if position(pool, session).await?.0 != cursor.generation {
                return Ok(Record::Reset);
            }
            cursor.since = cursor.through.take().context("missing stream checkpoint")?;
            cursor.item_after = cursor.since;
            cursor.operation_after = -1;
            cursor.items_done = false;
            return Ok(Record::Complete { cursor });
        }
        operations = values;
    }
    if position(pool, session).await?.0 != cursor.generation {
        return Ok(Record::Reset);
    }
    Ok(Record::Batch {
        items,
        operations,
        cursor,
    })
}

pub async fn operation_body(
    pool: &Pool,
    session: Uuid,
    turn: i64,
    pair: &str,
) -> anyhow::Result<Value> {
    operation_bodies(pool, session, turn, &[pair.to_string()])
        .await?
        .into_iter()
        .next()
        .context("operation not found")
}

pub async fn operation_bodies(
    pool: &Pool,
    session: Uuid,
    turn: i64,
    pairs: &[String],
) -> anyhow::Result<Vec<Value>> {
    ensure!(
        !pairs.is_empty() && pairs.len() <= 16,
        "request one to sixteen operation bodies"
    );
    let generation = position(pool, session).await?.0;
    let rows: Vec<Operation> = sqlx::query_as(
        "SELECT turn_id, operation_ord, pair_id, name, raw_name, operation_type, operation_category, \
         input, result_content, result_payload, result_is_error, is_error, is_pending, changed_at \
         FROM timeline_operations WHERE session_uuid = $1 AND turn_id = $2 AND pair_id = ANY($3)"
    ).bind(session).bind(turn).bind(pairs).fetch_all(pool).await?;
    let values = rows
        .into_iter()
        .map(|row| {
            let version = row.changed_at;
            let pair = super::build_operation_pair(row.row, &mut HashMap::new())?;
            Ok(
                json!({ "id": pair.id, "generation": generation, "body_version": version,
            "input": pair.input, "result": pair.result }),
            )
        })
        .collect::<anyhow::Result<_>>()?;
    ensure!(
        position(pool, session).await?.0 == generation,
        "projection changed during body read"
    );
    Ok(values)
}

pub async fn digest(pool: &Pool, session: Uuid, turn: i64) -> anyhow::Result<String> {
    let generation = position(pool, session).await?.0;
    let row = super::load_projected_turn_row(pool, session, turn, false)
        .await?
        .context("turn not found")?;
    if !row.markdown.is_empty() {
        return Ok(row.markdown);
    }
    let mut items = super::load_projected_items(pool, session, &[turn]).await?;
    let cursor = Cursor {
        turn_id: turn,
        generation: String::new(),
        since: -1,
        through: None,
        item_after: -1,
        operation_after: -1,
        items_done: true,
    };
    let mut cursor = cursor;
    let mut pairs = Vec::new();
    loop {
        let (values, last) = load_operations(pool, session, turn, &cursor, None).await?;
        if values.is_empty() {
            break;
        }
        for value in values {
            pairs.push(serde_json::from_value(value)?);
        }
        cursor.operation_after = last;
    }
    ensure!(
        position(pool, session).await?.0 == generation,
        "projection changed during digest read"
    );
    Ok(super::compose_turn_markdown(
        row.user_prompt_text.as_deref(),
        &items.remove(&turn).unwrap_or_default(),
        &pairs,
    ))
}
