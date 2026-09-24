//! The incremental timeline writer: every tick projects what it ingested,
//! a transcript applied piecemeal ends where a rebuild ends, and an append
//! rewrites only the records it affects.

use sulion::ingest::{
    load_timeline_response, load_timeline_turn_view, project_batch, rebuild_session_projection,
    ProjectionFilters,
};

use super::*;

fn claude(fx: &Fixture, line: serde_json::Value) {
    fx.append(&line.to_string());
    fx.append("\n");
}

fn ts(second: u32) -> String {
    format!("2026-09-24T10:{:02}:{:02}Z", second / 60, second % 60)
}

async fn snapshot(pool: &db::Pool, session: Uuid) -> serde_json::Value {
    let filters = ProjectionFilters {
        show_bookkeeping: true,
        show_sidechain: true,
        ..Default::default()
    };
    let response = load_timeline_response(pool, session, &filters)
        .await
        .unwrap();
    serde_json::to_value(response.turns).unwrap()
}

async fn projected_through(pool: &db::Pool, session: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT projected_through FROM timeline_session_state WHERE session_uuid = $1",
    )
    .bind(session)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A transcript exercising prompts, streamed receipts, calls and late
/// results, a task notification and pre-prompt bookkeeping.
fn claude_transcript() -> Vec<serde_json::Value> {
    use serde_json::json;
    vec![
        json!({"type":"attachment","timestamp":ts(0),"uuid":"att-1"}),
        json!({"type":"user","timestamp":ts(1),"uuid":"u1","message":{"role":"user","content":"first task"}}),
        json!({"type":"assistant","timestamp":ts(2),"uuid":"a1","message":{"id":"m1","role":"assistant",
            "content":[{"type":"thinking","thinking":"plan"}],"usage":{"input_tokens":10,"output_tokens":1}}}),
        json!({"type":"assistant","timestamp":ts(3),"uuid":"a2","message":{"id":"m1","role":"assistant",
            "content":[{"type":"text","text":"reading"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"src/lib.rs"}}],
            "usage":{"input_tokens":10,"output_tokens":7}}}),
        json!({"type":"user","timestamp":ts(4),"uuid":"r1","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t1","content":"fn main() {}"}]}}),
        json!({"type":"assistant","timestamp":ts(5),"uuid":"a3","message":{"id":"m2","role":"assistant",
            "content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"cargo test"}}],
            "usage":{"input_tokens":20,"output_tokens":3}}}),
        json!({"type":"user","timestamp":ts(6),"uuid":"n1","message":{"role":"user","content":
            "<task-notification>\n<task-id>bg-1</task-id>\n<status>completed</status>\n</task-notification>"}}),
        json!({"type":"user","timestamp":ts(7),"uuid":"u2","message":{"role":"user","content":"second task"}}),
        json!({"type":"user","timestamp":ts(8),"uuid":"r2","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t2","content":"1 failed","is_error":true}]}}),
        json!({"type":"assistant","timestamp":ts(9),"uuid":"a4","message":{"id":"m3","role":"assistant",
            "content":[{"type":"text","text":"the earlier test failed"}],"usage":{"input_tokens":30,"output_tokens":5}}}),
        json!({"type":"assistant","timestamp":ts(10),"uuid":"a5","message":{"id":"m2","role":"assistant",
            "content":[],"usage":{"input_tokens":20,"output_tokens":9}}}),
    ]
}

#[tokio::test]
async fn each_tick_projects_what_it_ingested() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let ingester = Ingester::new();
    for line in claude_transcript().into_iter().take(4) {
        claude(&fx, line);
        ingester.tick(&pool, &fx.config()).await.unwrap();
        let (latest, projected): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT MAX(byte_offset) FROM events WHERE session_uuid = $1), \
                    projected_through \
               FROM timeline_session_state WHERE session_uuid = $1",
        )
        .bind(fx.session_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(projected, latest);
    }
}

#[tokio::test]
async fn a_transcript_applied_line_by_line_ends_where_a_rebuild_ends() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    for line in claude_transcript() {
        claude(&fx, line);
        // A fresh ingester each time: nothing carries over in memory.
        Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    }
    let incremental = snapshot(&pool, fx.session_uuid).await;
    rebuild_session_projection(&pool, fx.session_uuid)
        .await
        .unwrap();
    assert_eq!(snapshot(&pool, fx.session_uuid).await, incremental);

    // The late failure completed the call in the first turn, and the
    // revised receipt replaced that turn's earlier contribution.
    let turns = incremental.as_array().unwrap();
    assert_eq!(turns.len(), 2);
    let bash = &turns[0]["tool_pairs"][1];
    assert_eq!(bash["id"], "t2");
    assert_eq!(bash["is_pending"], false);
    assert_eq!(bash["is_error"], true);
    assert_eq!(turns[0]["input_tokens"], 30);
    assert_eq!(turns[0]["output_tokens"], 16);
    assert_eq!(turns[1]["input_tokens"], 30);
    assert!(turns[0]["markdown"]
        .as_str()
        .unwrap()
        .contains("**Tool:** `bash` `cargo test` _(error)_"));
}

#[tokio::test]
async fn single_event_batches_end_where_a_rebuild_ends() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    for line in claude_transcript() {
        claude(&fx, line);
    }
    Ingester::new().tick(&pool, &fx.config()).await.unwrap();
    let whole = snapshot(&pool, fx.session_uuid).await;

    sqlx::query("UPDATE timeline_session_state SET projection_version = 0 WHERE session_uuid = $1")
        .bind(fx.session_uuid)
        .execute(&pool)
        .await
        .unwrap();
    let mut batches = 0;
    while project_batch(&pool, fx.session_uuid, 1).await.unwrap().more {
        batches += 1;
    }
    assert_eq!(batches, event_count(&pool, fx.session_uuid).await);
    assert_eq!(snapshot(&pool, fx.session_uuid).await, whole);
}

#[tokio::test]
async fn appending_to_a_long_turn_rewrites_only_the_new_records() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    claude(
        &fx,
        serde_json::json!({"type":"user","timestamp":ts(0),"uuid":"u1","message":{"role":"user","content":"many calls"}}),
    );
    for n in 0..300u32 {
        claude(
            &fx,
            serde_json::json!({"type":"assistant","timestamp":ts(1),"message":{"role":"assistant",
                "content":[{"type":"tool_use","id":format!("c{n}"),"name":"Bash","input":{"command":"true"}}]}}),
        );
        claude(
            &fx,
            serde_json::json!({"type":"user","timestamp":ts(1),"message":{"role":"user","content":[
                {"type":"tool_result","tool_use_id":format!("c{n}"),"content":"ok"}]}}),
        );
    }
    let ingester = Ingester::new();
    while event_count(&pool, fx.session_uuid).await < 601 {
        ingester.tick(&pool, &fx.config()).await.unwrap();
    }
    let before = projected_through(&pool, fx.session_uuid).await;

    claude(
        &fx,
        serde_json::json!({"type":"assistant","timestamp":ts(2),"message":{"role":"assistant",
            "content":[{"type":"tool_use","id":"tail","name":"Bash","input":{"command":"true"}}]}}),
    );
    ingester.tick(&pool, &fx.config()).await.unwrap();
    claude(
        &fx,
        serde_json::json!({"type":"user","timestamp":ts(3),"message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"tail","content":"done"}]}}),
    );
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let changed = |sql: &'static str| {
        let pool = pool.clone();
        let session = fx.session_uuid;
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(session)
                .bind(before)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        changed(
            "SELECT COUNT(*) FROM timeline_operations WHERE session_uuid = $1 AND changed_at > $2"
        )
        .await,
        1
    );
    assert_eq!(
        changed("SELECT COUNT(*) FROM timeline_items WHERE session_uuid = $1 AND byte_offset > $2")
            .await,
        1
    );
    let (pending, operations): (bool, i32) = sqlx::query_as(
        "SELECT o.is_pending, t.operation_count FROM timeline_operations o \
           JOIN timeline_turns t USING (session_uuid, turn_id) \
          WHERE o.session_uuid = $1 AND o.pair_id = 'tail'",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending);
    assert_eq!(operations, 301);
}

/// An open view reads the turn once, then only what changed: merging the
/// delta into the first read reproduces a whole read.
#[tokio::test]
async fn a_detail_read_since_an_offset_returns_only_changed_records() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    let transcript = claude_transcript();
    for line in transcript.iter().take(5).cloned() {
        claude(&fx, line);
    }
    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let turn_id: i64 = sqlx::query_scalar(
        "SELECT turn_id FROM timeline_turns WHERE session_uuid = $1 ORDER BY turn_ord LIMIT 1",
    )
    .bind(fx.session_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let filters = ProjectionFilters::default();
    let first = load_timeline_turn_view(&pool, fx.session_uuid, turn_id, &filters, None)
        .await
        .unwrap()
        .unwrap();

    for line in transcript.iter().skip(5).take(2).cloned() {
        claude(&fx, line);
    }
    ingester.tick(&pool, &fx.config()).await.unwrap();
    let delta = load_timeline_turn_view(
        &pool,
        fx.session_uuid,
        turn_id,
        &filters,
        Some(first.through),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(delta.through > first.through);
    assert_eq!(
        delta
            .turn
            .tool_pairs
            .iter()
            .map(|pair| pair.id.as_str())
            .collect::<Vec<_>>(),
        vec!["t2"],
        "only the new call is sent",
    );
    assert!(delta
        .turn
        .items
        .iter()
        .all(|item| item.offset > first.through));

    let mut items: Vec<serde_json::Value> = first
        .turn
        .items
        .iter()
        .chain(&delta.turn.items)
        .map(|item| serde_json::to_value(item).unwrap())
        .collect();
    items.dedup();
    let whole = load_timeline_turn_view(&pool, fx.session_uuid, turn_id, &filters, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::Value::Array(items),
        serde_json::to_value(&whole.turn.items).unwrap()
    );
    assert_eq!(whole.turn.event_count, delta.turn.event_count);
}

#[tokio::test]
async fn a_busy_file_admits_a_bounded_number_of_lines_per_tick() {
    let pool = fresh_pool().await;
    let busy = Fixture::new();
    let quiet_session = Uuid::new_v4();
    let quiet_path = busy
        .root
        .path()
        .join(&busy.project_hash)
        .join(format!("{quiet_session}.jsonl"));
    let lines = sulion::ingest::ADMIT_LINES_PER_TICK + 500;
    for n in 0..lines {
        claude(
            &busy,
            serde_json::json!({"type":"assistant","timestamp":ts(1),"message":{"role":"assistant",
                "content":[{"type":"text","text":format!("line {n}")}]}}),
        );
    }
    std::fs::write(
        &quiet_path,
        format!(
            "{}\n",
            serde_json::json!({"type":"user","timestamp":ts(0),"uuid":"q1","message":{"role":"user","content":"quiet"}})
        ),
    )
    .unwrap();

    let ingester = Ingester::new();
    let first = ingester.tick(&pool, &busy.config()).await.unwrap();
    assert!(first.backlogged);
    let admitted = event_count(&pool, busy.session_uuid).await;
    assert!(admitted > 0 && admitted <= sulion::ingest::ADMIT_LINES_PER_TICK as i64);
    assert_eq!(event_count(&pool, quiet_session).await, 1);
    let quiet_turns: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM timeline_turns WHERE session_uuid = $1")
            .bind(quiet_session)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(quiet_turns, 1);

    while event_count(&pool, busy.session_uuid).await < lines as i64 {
        ingester.tick(&pool, &busy.config()).await.unwrap();
    }
}

#[tokio::test]
async fn a_replaced_transcript_is_rebuilt_from_its_events() {
    let pool = fresh_pool().await;
    let fx = Fixture::new();
    for line in claude_transcript().into_iter().take(5) {
        claude(&fx, line);
    }
    let ingester = Ingester::new();
    ingester.tick(&pool, &fx.config()).await.unwrap();

    // Replaced by a shorter file. Its first line lands on an existing offset
    // and is ignored; the next starts at an offset the timeline has passed.
    std::fs::write(fx.jsonl_path(), "").unwrap();
    ingester.tick(&pool, &fx.config()).await.unwrap();
    claude(
        &fx,
        serde_json::json!({"type":"file-history-snapshot","timestamp":ts(19)}),
    );
    claude(
        &fx,
        serde_json::json!({"type":"user","timestamp":ts(20),"uuid":"x1","message":{"role":"user","content":"replacement prompt"}}),
    );
    ingester.tick(&pool, &fx.config()).await.unwrap();

    let incremental = snapshot(&pool, fx.session_uuid).await;
    rebuild_session_projection(&pool, fx.session_uuid)
        .await
        .unwrap();
    assert_eq!(snapshot(&pool, fx.session_uuid).await, incremental);
    assert!(incremental
        .as_array()
        .unwrap()
        .iter()
        .any(|turn| turn["preview"] == "replacement prompt"));
}
