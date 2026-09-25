use super::*;
use serde_json::Value;
use sulion::ingest::turn_stream::{self, Cursor, Record, BATCH_SIZE};

async fn fixture(h: &Harness, count: i64) -> Uuid {
    let pool = &h.state.pool;
    let session = Uuid::new_v4();
    sqlx::query("INSERT INTO claude_sessions (session_uuid, agent) VALUES ($1, 'claude-code')")
        .bind(session)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO timeline_session_state (session_uuid, projected_through) VALUES ($1, $2)",
    )
    .bind(session)
    .bind(count)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO timeline_turns (session_uuid, turn_id, turn_ord, preview, user_prompt_text, \
        start_timestamp, end_timestamp, duration_ms, event_count, operation_count, thinking_count, markdown) \
        VALUES ($1, 0, 0, 'large turn', 'large turn', NOW(), NOW(), 0, $2, 1, 0, '')")
        .bind(session).bind(count as i32).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO timeline_items (session_uuid, turn_id, byte_offset, body) \
        SELECT $1, 0, i, jsonb_build_object('kind','assistant','thinking','[]'::jsonb, \
        'items', jsonb_build_array(jsonb_build_object('kind','text','text','message ' || i))) \
        FROM generate_series(1, $2::bigint) i",
    )
    .bind(session)
    .bind(count)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO timeline_operations (session_uuid, turn_id, operation_ord, pair_id, name, \
        operation_type, input, result_content, changed_at) \
        VALUES ($1, 0, 0, 'tool', 'bash', 'bash', jsonb_build_object('command', 'echo hello'), repeat('x', 1000000), 1)")
        .bind(session).execute(pool).await.unwrap();
    session
}

async fn start(h: &Harness, session: Uuid, cursor: Option<Cursor>) -> Cursor {
    match turn_stream::begin(&h.state.pool, session, 0, cursor)
        .await
        .unwrap()
    {
        Record::Header { cursor, .. } => cursor,
        _ => panic!("expected header"),
    }
}

async fn drain(h: &Harness, session: Uuid, mut cursor: Cursor) -> (Cursor, Vec<i64>, Vec<Value>) {
    let mut offsets = Vec::new();
    let mut operations = Vec::new();
    loop {
        match turn_stream::next(&h.state.pool, session, 0, cursor)
            .await
            .unwrap()
        {
            Record::Batch {
                cursor: next,
                items,
                operations: ops,
            } => {
                assert!(items.len() <= BATCH_SIZE as usize);
                offsets.extend(items.into_iter().map(|i| i.offset));
                operations.extend(ops);
                cursor = next;
            }
            Record::Complete { cursor } => return (cursor, offsets, operations),
            _ => panic!("unexpected stream record"),
        }
    }
}

#[tokio::test]
async fn bounded_turn_batches_resume_and_deliver_late_results() {
    let h = Harness::new().await;
    let session = fixture(&h, 150).await;
    let cursor = start(&h, session, None).await;
    let first = turn_stream::next(&h.state.pool, session, 0, cursor)
        .await
        .unwrap();
    let Record::Batch { items, cursor, .. } = first else {
        panic!("batch")
    };
    assert_eq!(items.len(), 64);
    assert_eq!(
        cursor.since, -1,
        "partial reads must not advance synchronization"
    );
    // Resume a disconnected request and mutate an already existing operation.
    let cursor = start(&h, session, Some(cursor)).await;
    sqlx::query(
        "UPDATE timeline_operations SET changed_at = 151, is_error = TRUE WHERE session_uuid = $1",
    )
    .bind(session)
    .execute(&h.state.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE timeline_session_state SET projected_through = 151 WHERE session_uuid = $1",
    )
    .bind(session)
    .execute(&h.state.pool)
    .await
    .unwrap();
    let (complete, rest, ops) = drain(&h, session, cursor).await;
    assert_eq!(rest, (65..=150).collect::<Vec<_>>());
    assert_eq!(
        complete.since, 150,
        "checkpoint remains the initial read offset"
    );
    assert!(ops[0]["is_error"].as_bool().unwrap());
    assert!(ops[0]["result"].is_null());
    assert!(serde_json::to_vec(&ops).unwrap().len() < 2000);
    let next = start(&h, session, Some(complete)).await;
    let (_, items, ops) = drain(&h, session, next).await;
    assert!(items.is_empty());
    assert_eq!(ops[0]["body_version"], 151);
    let body = turn_stream::operation_body(&h.state.pool, session, 0, "tool")
        .await
        .unwrap();
    assert_eq!(body["result"]["content"].as_str().unwrap().len(), 1_000_000);
}

#[tokio::test]
async fn turn_stream_detects_rebuild_and_archive_and_serves_digest() {
    let h = Harness::new().await;
    let session = fixture(&h, 2).await;
    let old = start(&h, session, None).await;
    sqlx::query(
        "UPDATE timeline_session_state SET generation = gen_random_uuid() WHERE session_uuid = $1",
    )
    .bind(session)
    .execute(&h.state.pool)
    .await
    .unwrap();
    assert!(matches!(
        turn_stream::next(&h.state.pool, session, 0, old.clone())
            .await
            .unwrap(),
        Record::Reset
    ));
    assert!(matches!(
        turn_stream::begin(&h.state.pool, session, 0, Some(old))
            .await
            .unwrap(),
        Record::Header { reset: true, .. }
    ));
    let digest = turn_stream::digest(&h.state.pool, session, 0)
        .await
        .unwrap();
    assert!(digest.contains("message 1") && digest.contains("message 2"));
    assert!(digest.len() < 1000);
    let cursor = start(&h, session, None).await;
    sqlx::query(
        "UPDATE claude_sessions SET purged_at = NOW(), archived_at = NOW() WHERE session_uuid = $1",
    )
    .bind(session)
    .execute(&h.state.pool)
    .await
    .unwrap();
    assert!(matches!(
        turn_stream::next(&h.state.pool, session, 0, cursor)
            .await
            .unwrap(),
        Record::Reset
    ));
}

#[tokio::test]
async fn http_turn_stream_flushes_header_before_reading_items() {
    let h = Harness::new().await;
    let session = fixture(&h, 150).await;
    // Blocking item reads establishes a deterministic first-byte boundary.
    let mut lock = h.state.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE timeline_items IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let mut response = h
        .client
        .get(format!("{}/api/timeline/{session}/turns/0/stream", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let first = tokio::time::timeout(Duration::from_secs(2), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(std::str::from_utf8(&first)
        .unwrap()
        .contains("\"kind\":\"header\""));
    assert!(!std::str::from_utf8(&first)
        .unwrap()
        .contains("\"kind\":\"complete\""));
    lock.rollback().await.unwrap();
    let rest = response.text().await.unwrap();
    assert!(rest.contains("\"kind\":\"complete\""));
    assert!(
        !rest.contains(&"x".repeat(1000)),
        "collapsed output never enters stream"
    );
}

#[tokio::test]
async fn compare_complete_and_streamed_turn_reads() {
    let h = Harness::new().await;
    for count in [8, 4096] {
        let session = fixture(&h, count).await;
        let started = std::time::Instant::now();
        let full = sulion::ingest::load_timeline_turn_view(
            &h.state.pool,
            session,
            0,
            &Default::default(),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let full_bytes = serde_json::to_vec(&full.turn).unwrap().len();
        let full_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = std::time::Instant::now();
        let header = turn_stream::begin(&h.state.pool, session, 0, None)
            .await
            .unwrap();
        let mut bytes = serde_json::to_vec(&header).unwrap().len() + 1;
        let Record::Header { mut cursor, .. } = header else {
            panic!("header")
        };
        let mut first_ms = None;
        let mut batches = 0;
        loop {
            let record = turn_stream::next(&h.state.pool, session, 0, cursor)
                .await
                .unwrap();
            bytes += serde_json::to_vec(&record).unwrap().len() + 1;
            match record {
                Record::Batch { cursor: next, .. } => {
                    first_ms.get_or_insert_with(|| started.elapsed().as_secs_f64() * 1000.0);
                    batches += 1;
                    cursor = next;
                }
                Record::Complete { .. } => break,
                _ => panic!("unexpected record"),
            }
        }
        println!("turn-read fixture: items={count}, full_bytes={full_bytes}, stream_bytes={bytes}, full_ms={full_ms:.2}, first_batch_ms={:.2}, stream_total_ms={:.2}, batches={batches}",
            first_ms.unwrap(), started.elapsed().as_secs_f64() * 1000.0);
        assert!(bytes < full_bytes, "collapsed output is excluded");
    }
}
