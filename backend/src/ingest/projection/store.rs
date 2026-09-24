//! The reducer's view of committed rows, read inside the batch transaction.

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::ingest::canonical::OperationCategory;
use crate::ingest::timeline::reduce::{Backend, MessageUsage, OpRow, TurnRow};

pub(super) struct PgBackend {
    pub(super) tx: Transaction<'static, Postgres>,
    pub(super) session_uuid: Uuid,
}

#[derive(FromRow)]
struct TurnDbRow {
    turn_id: i64,
    turn_ord: i32,
    is_sidechain_turn: bool,
    preview: String,
    user_prompt_text: Option<String>,
    prompt_event_uuid: Option<String>,
    start_timestamp: DateTime<Utc>,
    end_timestamp: DateTime<Utc>,
    duration_ms: i64,
    event_count: i32,
    operation_count: i32,
    thinking_count: i32,
    has_errors: bool,
    input_tokens: i64,
    output_tokens: i64,
    usage_baseline_input: i64,
    usage_baseline_output: i64,
}

#[derive(FromRow)]
struct OpDbRow {
    turn_id: i64,
    operation_ord: i32,
    pair_id: String,
    name: String,
    raw_name: Option<String>,
    operation_type: Option<String>,
    operation_category: Option<String>,
    input: Option<Value>,
    result_content: Option<String>,
    result_payload: Option<Value>,
    result_is_error: bool,
    is_error: bool,
    is_pending: bool,
    call_offset: Option<i64>,
    call_at: Option<DateTime<Utc>>,
    changed_at: i64,
    call_error: bool,
    running_cell: Option<String>,
    finished_at: Option<DateTime<Utc>>,
}

impl From<OpDbRow> for OpRow {
    fn from(row: OpDbRow) -> Self {
        OpRow {
            turn_id: row.turn_id,
            operation_ord: row.operation_ord,
            pair_id: row.pair_id,
            name: row.name,
            raw_name: row.raw_name,
            operation_type: row.operation_type,
            category: row
                .operation_category
                .as_deref()
                .and_then(OperationCategory::parse),
            input: row.input,
            result_content: row.result_content,
            result_payload: row.result_payload,
            result_is_error: row.result_is_error,
            is_error: row.is_error,
            is_pending: row.is_pending,
            call_offset: row.call_offset.unwrap_or(-1),
            call_at: row.call_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            changed_at: row.changed_at,
            call_error: row.call_error,
            running_cell: row.running_cell,
            finished_at: row.finished_at,
        }
    }
}

const OP_COLUMNS: &str = "turn_id, operation_ord, pair_id, name, raw_name, operation_type, \
     operation_category, input, result_content, result_payload, result_is_error, is_error, \
     is_pending, call_offset, call_at, changed_at, call_error, running_cell, finished_at";

type OpQuery<'q> = sqlx::query::QueryAs<'q, Postgres, OpDbRow, sqlx::postgres::PgArguments>;

impl PgBackend {
    /// The session's operations matching `filter`, whose parameters start
    /// at `$2`.
    async fn ops(
        &mut self,
        filter: &str,
        bind: impl FnOnce(OpQuery<'_>) -> OpQuery<'_>,
    ) -> anyhow::Result<Vec<OpRow>> {
        let sql = format!(
            "SELECT {OP_COLUMNS} FROM timeline_operations WHERE session_uuid = $1 AND {filter}"
        );
        let rows = bind(sqlx::query_as::<_, OpDbRow>(&sql).bind(self.session_uuid))
            .fetch_all(&mut *self.tx)
            .await
            .context("load timeline operations")?;
        Ok(rows.into_iter().map(OpRow::from).collect())
    }
}

impl Backend for PgBackend {
    async fn turn(&mut self, turn_id: i64) -> anyhow::Result<Option<TurnRow>> {
        let row: Option<TurnDbRow> = sqlx::query_as(
            "SELECT turn_id, turn_ord, is_sidechain_turn, preview, user_prompt_text, \
                    prompt_event_uuid, start_timestamp, end_timestamp, duration_ms, event_count, \
                    operation_count, thinking_count, has_errors, input_tokens, output_tokens, \
                    usage_baseline_input, usage_baseline_output \
               FROM timeline_turns WHERE session_uuid = $1 AND turn_id = $2",
        )
        .bind(self.session_uuid)
        .bind(turn_id)
        .fetch_optional(&mut *self.tx)
        .await
        .context("load timeline turn")?;
        Ok(row.map(|row| TurnRow {
            turn_id: row.turn_id,
            turn_ord: row.turn_ord,
            is_sidechain: row.is_sidechain_turn,
            preview: row.preview,
            user_prompt_text: row.user_prompt_text,
            prompt_event_uuid: row.prompt_event_uuid,
            start_timestamp: row.start_timestamp,
            end_timestamp: row.end_timestamp,
            duration_ms: row.duration_ms,
            event_count: row.event_count,
            operation_count: row.operation_count,
            thinking_count: row.thinking_count,
            has_errors: row.has_errors,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            usage_baseline_input: row.usage_baseline_input,
            usage_baseline_output: row.usage_baseline_output,
        }))
    }

    async fn ops_by_pair(&mut self, pair_id: &str, before: i64) -> anyhow::Result<Vec<OpRow>> {
        let pair_id = pair_id.to_string();
        self.ops("pair_id = $2 AND call_offset < $3", move |query| {
            query.bind(pair_id).bind(before)
        })
        .await
    }

    async fn prompt_seen(&mut self, event_uuid: &str) -> anyhow::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM timeline_turns \
                             WHERE session_uuid = $1 AND prompt_event_uuid = $2)",
        )
        .bind(self.session_uuid)
        .bind(event_uuid)
        .fetch_one(&mut *self.tx)
        .await
        .context("check projected prompt")
    }

    async fn message_usage(&mut self, message_id: &str) -> anyhow::Result<Option<MessageUsage>> {
        let row: Option<(i64, i64, i64)> = sqlx::query_as(
            "SELECT turn_id, input_tokens, output_tokens FROM timeline_message_usage \
              WHERE session_uuid = $1 AND message_id = $2",
        )
        .bind(self.session_uuid)
        .bind(message_id)
        .fetch_optional(&mut *self.tx)
        .await
        .context("load response usage")?;
        Ok(
            row.map(|(turn_id, input_tokens, output_tokens)| MessageUsage {
                turn_id,
                input_tokens,
                output_tokens,
            }),
        )
    }

    async fn exec_candidates(
        &mut self,
        before: i64,
        started: Option<DateTime<Utc>>,
    ) -> anyhow::Result<Vec<OpRow>> {
        // A millisecond of slack either side; the reducer applies the window.
        self.ops(
            "(raw_name = 'exec' OR raw_name LIKE '%.exec') AND call_offset < $2 \
             AND ( ($3::TIMESTAMPTZ IS NULL AND finished_at IS NULL) \
                OR ($3::TIMESTAMPTZ IS NOT NULL \
                    AND call_at <= $3 + INTERVAL '1 millisecond' \
                    AND (finished_at IS NULL OR finished_at >= $3 - INTERVAL '1 millisecond')) )",
            move |query| query.bind(before).bind(started),
        )
        .await
    }

    async fn ops_by_running_cell(&mut self, cell: &str) -> anyhow::Result<Vec<OpRow>> {
        let cell = cell.to_string();
        self.ops("running_cell = $2", move |query| query.bind(cell))
            .await
    }
}
