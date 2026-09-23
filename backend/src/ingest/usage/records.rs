use std::collections::{BTreeMap, HashSet};

use chrono::NaiveDate;
use sqlx::QueryBuilder;

use super::*;

pub(super) fn extract_response(payload: &Value) -> Option<UsageUpdate> {
    let response_id = payload
        .get("response_id")?
        .as_str()
        .filter(|id| !id.is_empty())?;
    let usage = payload.get("usage").filter(|usage| usage.is_object())?;
    for key in ["input_tokens", "output_tokens"] {
        usage.get(key)?.as_i64().filter(|tokens| *tokens >= 0)?;
    }
    let cached = token_at(usage, "cached_input_tokens");
    let writes = token_at(usage, "cache_write_input_tokens");
    Some(UsageUpdate {
        mode: UsageMode::Delta,
        message_id: Some(response_id.to_owned()),
        model: string_at(payload, &["model"]).map(str::to_owned),
        input_tokens: token_at(usage, "input_tokens")
            .saturating_sub(cached)
            .saturating_sub(writes)
            .max(0),
        cached_input_tokens: cached,
        cache_write_input_tokens: writes,
        cache_write_1h_input_tokens: 0,
        output_tokens: token_at(usage, "output_tokens"),
        reasoning_output_tokens: token_at(usage, "reasoning_output_tokens"),
        total_tokens: total_token_count(usage),
        // Compaction responses are spend, not the post-compaction context.
        // Continue to take context pressure from token_count.
        context_tokens: None,
        model_context_window: None,
    })
}

pub(super) async fn claim_response(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    response_id: &str,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(
        "INSERT INTO agent_usage_responses (session_uuid, response_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(session_uuid)
    .bind(response_id)
    .execute(&mut **tx)
    .await?
    .rows_affected()
        > 0)
}

pub(super) async fn update_context(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    byte_offset: i64,
    observed_at: DateTime<Utc>,
    usage: &UsageUpdate,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_session_usage SET \
            context_tokens = COALESCE($2, context_tokens), \
            model_context_window = COALESCE($3, model_context_window), \
            last_byte_offset = $4, observed_at = $5, updated_at = NOW() \
         WHERE session_uuid = $1 AND last_byte_offset < $4",
    )
    .bind(session_uuid)
    .bind(usage.context_tokens)
    .bind(usage.model_context_window)
    .bind(byte_offset)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;
    snapshot_daily(tx, session_uuid, observed_at).await
}

#[derive(sqlx::FromRow)]
struct ReplayEvent {
    byte_offset: i64,
    timestamp: DateTime<Utc>,
    payload: Value,
}

#[derive(Default)]
struct Replay {
    model: Option<String>,
    total: Option<UsageUpdate>,
    responses: HashSet<String>,
    days: BTreeMap<NaiveDate, (i64, DateTime<Utc>, UsageUpdate, bool)>,
    models: BTreeMap<(NaiveDate, String), UsageComponents>,
}

impl Replay {
    fn observe(&mut self, event: ReplayEvent) {
        if super::super::canonical::codex_record_kind(&event.payload) == Some("turn_context") {
            if let Some(model) = string_at(&event.payload, &["payload", "model"]) {
                self.model = Some(model.to_owned());
            }
            return;
        }
        let Some(usage) = extract_codex_usage(&event.payload) else {
            return;
        };
        let is_response = usage.mode == UsageMode::Delta;
        if is_response && !self.responses.insert(usage.message_id.clone().unwrap()) {
            return;
        }
        let context_only = !is_response && !self.responses.is_empty();
        if !context_only {
            let previous = self.total.as_ref().map(|total| StoredUsage {
                input_tokens: total.input_tokens,
                cached_input_tokens: total.cached_input_tokens,
                cache_write_input_tokens: total.cache_write_input_tokens,
                cache_write_1h_input_tokens: total.cache_write_1h_input_tokens,
                output_tokens: total.output_tokens,
                last_usage_message_id: total.message_id.clone(),
                last_byte_offset: -1,
                codex_response_records: !self.responses.is_empty(),
            });
            let delta = usage.daily_delta(previous.as_ref());
            let model = usage
                .model
                .as_ref()
                .or(self.model.as_ref())
                .cloned()
                .unwrap_or_else(|| "(unknown model)".to_owned());
            let day = self
                .models
                .entry((event.timestamp.date_naive(), model))
                .or_default();
            day.input_tokens += delta.input_tokens;
            day.cached_input_tokens += delta.cached_input_tokens;
            day.cache_write_input_tokens += delta.cache_write_input_tokens;
            day.cache_write_1h_input_tokens += delta.cache_write_1h_input_tokens;
            day.output_tokens += delta.output_tokens;
        }
        match &mut self.total {
            Some(total) => accumulate(total, &usage, context_only),
            None => self.total = Some(usage),
        }
        let mut total = self.total.clone().unwrap();
        total.mode = UsageMode::Cumulative;
        self.days.insert(
            event.timestamp.date_naive(),
            (
                event.byte_offset,
                event.timestamp,
                total,
                !self.responses.is_empty(),
            ),
        );
    }
}

fn accumulate(total: &mut UsageUpdate, next: &UsageUpdate, context_only: bool) {
    let context = next.context_tokens.or(total.context_tokens);
    let window = next.model_context_window.or(total.model_context_window);
    if !context_only {
        if next.mode == UsageMode::Cumulative {
            *total = next.clone();
        } else {
            total.input_tokens = total.input_tokens.saturating_add(next.input_tokens);
            total.cached_input_tokens = total
                .cached_input_tokens
                .saturating_add(next.cached_input_tokens);
            total.cache_write_input_tokens = total
                .cache_write_input_tokens
                .saturating_add(next.cache_write_input_tokens);
            total.cache_write_1h_input_tokens = total
                .cache_write_1h_input_tokens
                .saturating_add(next.cache_write_1h_input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(next.output_tokens);
            total.reasoning_output_tokens = total
                .reasoning_output_tokens
                .saturating_add(next.reasoning_output_tokens);
            total.total_tokens = total.total_tokens.saturating_add(next.total_tokens);
            total.message_id = next.message_id.clone();
        }
    }
    total.context_tokens = context;
    total.model_context_window = window;
}

/// Replace only sessions containing response records after the legacy rebuild.
/// Aggregate in memory and write daily rows in batches, avoiding a database
/// round trip for each historical response while the projection lock is held.
pub(super) async fn rebuild(tx: &mut Transaction<'_, Postgres>) -> Result<u64, sqlx::Error> {
    let sessions: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT session_uuid FROM events WHERE agent = 'codex' AND kind = 'token_usage_record'",
    ).fetch_all(&mut **tx).await?;
    let mut rebuilt = 0;
    for session in sessions {
        let events: Vec<ReplayEvent> = sqlx::query_as(
            "SELECT byte_offset, timestamp, \
                jsonb_build_object('type', COALESCE(payload->>'type', payload->>'kind'), \
                    'payload', CASE WHEN kind = 'turn_context' \
                        THEN jsonb_build_object('model', payload #> '{payload,model}') \
                        ELSE payload->'payload' END) AS payload \
             FROM events WHERE session_uuid = $1 \
               AND subtype IS DISTINCT FROM 'inherited_history' \
               AND kind IN ('token_count', 'token_usage_record', 'turn_context') \
             ORDER BY byte_offset",
        )
        .bind(session)
        .fetch_all(&mut **tx)
        .await?;
        let mut replay = Replay::default();
        for event in events {
            replay.observe(event);
        }
        if replay.responses.is_empty() {
            continue;
        }
        persist_replay(tx, session, replay).await?;
        rebuilt += 1;
    }
    Ok(rebuilt)
}

async fn persist_replay(
    tx: &mut Transaction<'_, Postgres>,
    session: Uuid,
    replay: Replay,
) -> Result<(), sqlx::Error> {
    for table in [
        "agent_session_usage",
        "agent_usage_daily",
        "agent_model_usage_daily",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE session_uuid = $1"))
            .bind(session)
            .execute(&mut **tx)
            .await?;
    }
    let mut days: Vec<_> = replay.days.values().collect();
    days.sort_by_key(|day| day.0);
    for (offset, timestamp, usage, modern) in days {
        store_usage(
            tx,
            session,
            TranscriptSource::Codex,
            *offset,
            *timestamp,
            usage,
            *modern,
        )
        .await?;
        snapshot_daily(tx, session, *timestamp).await?;
    }
    for ((date, model), usage) in replay.models {
        let timestamp = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        add_model_daily(tx, session, "codex", &model, timestamp, usage, None).await?;
    }
    let responses: Vec<_> = replay.responses.into_iter().collect();
    for chunk in responses.chunks(1000) {
        let mut query =
            QueryBuilder::new("INSERT INTO agent_usage_responses (session_uuid, response_id) ");
        query.push_values(chunk, |mut row, response| {
            row.push_bind(session).push_bind(response);
        });
        query.build().execute(&mut **tx).await?;
    }
    Ok(())
}
