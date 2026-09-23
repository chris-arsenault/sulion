use super::*;

pub(super) async fn apply_response(
    tx: &mut Transaction<'_, Postgres>,
    session_uuid: Uuid,
    byte_offset: i64,
    observed_at: DateTime<Utc>,
    usage: &UsageUpdate,
) -> Result<(), sqlx::Error> {
    let prior: Option<(DateTime<Utc>, Value)> = if let Some(id) = &usage.message_id {
        sqlx::query_as(
            "SELECT timestamp, payload FROM events \
             WHERE session_uuid=$1 AND agent='claude-code' AND kind='assistant' \
               AND payload #>> '{message,id}'=$2 AND byte_offset<$3 \
               AND jsonb_typeof(COALESCE(payload #> '{message,usage}', payload->'usage'))='object' \
             ORDER BY byte_offset DESC LIMIT 1",
        )
        .bind(session_uuid)
        .bind(id)
        .bind(byte_offset)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    let previous = prior
        .as_ref()
        .and_then(|(_, value)| extract_claude_usage(value));
    let mut delta = usage.clone();
    if let Some(previous) = &previous {
        delta.input_tokens -= previous.input_tokens;
        delta.cached_input_tokens -= previous.cached_input_tokens;
        delta.cache_write_input_tokens -= previous.cache_write_input_tokens;
        delta.cache_write_1h_input_tokens -= previous.cache_write_1h_input_tokens;
        delta.output_tokens -= previous.output_tokens;
        delta.reasoning_output_tokens -= previous.reasoning_output_tokens;
        delta.total_tokens -= previous.total_tokens;
    }
    sqlx::query(
        "INSERT INTO agent_session_usage (session_uuid,agent,last_byte_offset,observed_at) \
         VALUES ($1,'claude-code',$2,$3) ON CONFLICT DO NOTHING",
    )
    .bind(session_uuid)
    .bind(byte_offset)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE agent_session_usage SET input_tokens=input_tokens+$2, \
         cached_input_tokens=cached_input_tokens+$3,cache_write_input_tokens=cache_write_input_tokens+$4, \
         cache_write_1h_input_tokens=cache_write_1h_input_tokens+$5,output_tokens=output_tokens+$6, \
         reasoning_output_tokens=reasoning_output_tokens+$7,total_tokens=total_tokens+$8, \
         context_tokens=$9,model_context_window=COALESCE($10,model_context_window), \
         last_byte_offset=$11,observed_at=$12,last_usage_message_id=$13,updated_at=NOW() \
         WHERE session_uuid=$1",
    ).bind(session_uuid).bind(delta.input_tokens).bind(delta.cached_input_tokens)
        .bind(delta.cache_write_input_tokens).bind(delta.cache_write_1h_input_tokens)
        .bind(delta.output_tokens).bind(delta.reasoning_output_tokens).bind(delta.total_tokens)
        .bind(usage.context_tokens).bind(usage.model_context_window).bind(byte_offset)
        .bind(observed_at).bind(&usage.message_id).execute(&mut **tx).await?;

    // Remove the previous contribution before adding the replacement. Keeping
    // the full old/new values handles downward corrections and day/model moves.
    if let (Some((time, _)), Some(previous)) = (&prior, &previous) {
        sqlx::query(
            "UPDATE agent_model_usage_daily SET \
               input_tokens=input_tokens-$4, cached_input_tokens=cached_input_tokens-$5, \
               cache_write_input_tokens=cache_write_input_tokens-$6, \
               cache_write_1h_input_tokens=cache_write_1h_input_tokens-$7, \
               output_tokens=output_tokens-$8, updated_at=NOW() \
             WHERE session_uuid=$1 AND day=$2 AND model=$3",
        )
        .bind(session_uuid)
        .bind(time.date_naive())
        .bind(previous.model.as_deref().unwrap_or("(unknown model)"))
        .bind(previous.input_tokens)
        .bind(previous.cached_input_tokens)
        .bind(previous.cache_write_input_tokens)
        .bind(previous.cache_write_1h_input_tokens)
        .bind(previous.output_tokens)
        .execute(&mut **tx)
        .await?;
    }
    add_model_daily(
        tx,
        session_uuid,
        "claude-code",
        usage.model.as_deref().unwrap_or("(unknown model)"),
        observed_at,
        usage.daily_delta(None),
        usage.message_id.as_deref(),
    )
    .await?;

    if let (Some((time, _)), Some(previous)) = (&prior, &previous) {
        if time.date_naive() != observed_at.date_naive() || previous.model != usage.model {
            // Remove a bucket whose only response moved to another day/model.
            // Zero-token receipts still retain their bucket when they exist.
            sqlx::query(
                "WITH receipts AS (SELECT DISTINCT ON (COALESCE(payload #>> '{message,id}',byte_offset::TEXT)) \
                    timestamp,payload FROM events WHERE session_uuid=$1 AND agent='claude-code' \
                    AND kind='assistant' AND jsonb_typeof(COALESCE(payload #> '{message,usage}',payload->'usage'))='object' \
                    ORDER BY COALESCE(payload #>> '{message,id}',byte_offset::TEXT),byte_offset DESC) \
                 DELETE FROM agent_model_usage_daily d WHERE d.session_uuid=$1 AND NOT EXISTS \
                    (SELECT 1 FROM receipts r WHERE (r.timestamp AT TIME ZONE 'UTC')::DATE=d.day \
                     AND COALESCE(r.payload #>> '{message,model}','(unknown model)')=d.model)",
            ).bind(session_uuid).execute(&mut **tx).await?;
        }
    }
    if prior
        .as_ref()
        .is_some_and(|(time, _)| time.date_naive() != observed_at.date_naive())
    {
        // A response crossing midnight moves its contribution to the latest
        // receipt's day, exactly as the rebuild's DISTINCT ON does.
        rebuild_daily_snapshots(tx, session_uuid).await?;
    } else {
        snapshot_daily(tx, session_uuid, observed_at).await?;
    }
    Ok(())
}

async fn rebuild_daily_snapshots(
    tx: &mut Transaction<'_, Postgres>,
    session: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_usage_daily WHERE session_uuid=$1")
        .bind(session)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO agent_usage_daily \
         (day,session_uuid,agent,input_tokens,cached_input_tokens,cache_write_input_tokens, \
          cache_write_1h_input_tokens,output_tokens,reasoning_output_tokens,total_tokens) \
         SELECT day,$1,'claude-code',SUM(input) OVER w,SUM(cached) OVER w, \
            SUM(writes) OVER w,SUM(hour_writes) OVER w,SUM(output) OVER w,0, \
            SUM(input+cached+writes+hour_writes+output) OVER w \
         FROM (SELECT day,SUM(input_tokens) AS input,SUM(cached_input_tokens) AS cached, \
            SUM(cache_write_input_tokens) AS writes,SUM(cache_write_1h_input_tokens) AS hour_writes, \
            SUM(output_tokens) AS output FROM agent_model_usage_daily \
            WHERE session_uuid=$1 GROUP BY day) daily \
         WINDOW w AS (ORDER BY day ROWS UNBOUNDED PRECEDING)",
    ).bind(session).execute(&mut **tx).await?;
    Ok(())
}
