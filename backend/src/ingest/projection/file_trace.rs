use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

use crate::db::Pool;

#[derive(Debug, Clone)]
pub struct RepoFileTraceTouch {
    pub pty_session_id: Option<Uuid>,
    pub session_uuid: Uuid,
    pub session_agent: Option<String>,
    pub session_label: Option<String>,
    pub session_state: Option<String>,
    pub turn_id: i64,
    pub turn_preview: String,
    pub turn_timestamp: DateTime<Utc>,
    pub operation_type: Option<String>,
    pub operation_category: Option<String>,
    /// Stable id of the tool call that did the touch, when the touch
    /// was attached to one. Lets the client jump the user straight to
    /// the specific tool row inside the turn, not just the turn head.
    pub pair_id: Option<String>,
    pub touch_kind: String,
    pub is_write: bool,
}

#[derive(FromRow)]
struct TraceRow {
    pty_session_id: Option<Uuid>,
    session_uuid: Uuid,
    session_agent: Option<String>,
    session_label: Option<String>,
    session_state: Option<String>,
    turn_id: i64,
    turn_preview: String,
    turn_timestamp: DateTime<Utc>,
    operation_type: Option<String>,
    operation_category: Option<String>,
    pair_id: Option<String>,
    touch_kind: String,
    is_write: bool,
}

pub async fn load_repo_file_trace(
    pool: &Pool,
    repo_name: &str,
    repo_rel_path: &str,
) -> anyhow::Result<Vec<RepoFileTraceTouch>> {
    let rows: Vec<TraceRow> = sqlx::query_as(
        "SELECT cs.pty_session_id AS pty_session_id, \
                tf.session_uuid, \
                cs.agent AS session_agent, \
                ps.label AS session_label, \
                ps.state AS session_state, \
                tf.turn_id, \
                tt.preview AS turn_preview, \
                tt.start_timestamp AS turn_timestamp, \
                op.operation_type AS operation_type, \
                op.operation_category AS operation_category, \
                op.pair_id AS pair_id, \
                tf.touch_kind, \
                tf.is_write \
           FROM timeline_file_touches tf \
           JOIN timeline_turns tt \
             ON tt.session_uuid = tf.session_uuid AND tt.turn_id = tf.turn_id \
           LEFT JOIN timeline_operations op \
             ON op.session_uuid = tf.session_uuid \
            AND op.turn_id = tf.turn_id \
            AND op.operation_ord = tf.operation_ord \
           JOIN claude_sessions cs ON cs.session_uuid = tf.session_uuid \
           LEFT JOIN pty_sessions ps ON ps.id = cs.pty_session_id \
          WHERE tf.repo_name = $1 AND tf.repo_rel_path = $2 \
          ORDER BY tt.start_timestamp DESC, tf.turn_id DESC, tf.touch_ord ASC",
    )
    .bind(repo_name)
    .bind(repo_rel_path)
    .fetch_all(pool)
    .await
    .context("load repo file trace")?;

    Ok(rows.into_iter().map(RepoFileTraceTouch::from).collect())
}

impl From<TraceRow> for RepoFileTraceTouch {
    fn from(row: TraceRow) -> Self {
        Self {
            pty_session_id: row.pty_session_id,
            session_uuid: row.session_uuid,
            session_agent: row.session_agent,
            session_label: row.session_label,
            session_state: row.session_state,
            turn_id: row.turn_id,
            turn_preview: row.turn_preview,
            turn_timestamp: row.turn_timestamp,
            operation_type: row.operation_type,
            operation_category: row.operation_category,
            pair_id: row.pair_id,
            touch_kind: row.touch_kind,
            is_write: row.is_write,
        }
    }
}
