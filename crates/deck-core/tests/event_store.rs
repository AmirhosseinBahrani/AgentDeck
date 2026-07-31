//! The durable event log.
//!
//! Its whole purpose is that the lossy broadcast path is allowed to drop events *because* this one
//! cannot. So the tests are about completeness and ordering under load, and about a bad row not
//! taking a transcript down with it.

use deck_core::bus::{Attribution, EventBus};
use deck_core::domain::event::{AgentEvent, EventEnvelope, ExitReason};
use deck_core::domain::ids::{Seq, SessionId};
use deck_core::store::{events, Store};
use std::time::Duration;

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

fn envelope(seq: u64, session: Option<SessionId>, event: AgentEvent) -> EventEnvelope {
    EventEnvelope {
        seq: Seq(seq),
        at_ms: 1_760_000_000_000 + seq as i64,
        session_id: session,
        agent_id: None,
        task_id: None,
        event,
    }
}

fn diag(n: u64) -> AgentEvent {
    AgentEvent::Diagnostic {
        message: format!("event {n}"),
    }
}

/// Feeds envelopes through the writer and waits for it to drain.
async fn write_all(store: &Store, envelopes: Vec<EventEnvelope>) {
    let (tx, rx) = tokio::sync::mpsc::channel(4096);
    let writer = tokio::spawn(events::run_writer(store.clone(), rx));

    for envelope in envelopes {
        tx.send(envelope).await.expect("send");
    }
    // Closing the channel makes the writer flush its partial batch and return, which is also the
    // shutdown path in production.
    drop(tx);
    writer.await.expect("writer");
}

#[tokio::test]
async fn every_event_survives_and_keeps_its_order() {
    let s = store().await;
    let envelopes: Vec<EventEnvelope> = (1..=250)
        .map(|n| envelope(n, Some(SessionId::new()), diag(n)))
        .collect();

    write_all(&s, envelopes).await;

    assert_eq!(events::count(&s).await.unwrap(), 250);

    let read = events::since(&s, Seq(0), 1000).await.unwrap();
    let seqs: Vec<u64> = read.iter().map(|e| e.seq.0).collect();
    assert_eq!(
        seqs,
        (1..=250).collect::<Vec<_>>(),
        "order must be preserved"
    );
}

#[tokio::test]
async fn a_partial_batch_is_flushed_rather_than_discarded_on_shutdown() {
    // 250 is two full batches plus a remainder; losing the remainder would silently truncate the
    // log every time the app closed.
    let s = store().await;
    write_all(&s, (1..=250).map(|n| envelope(n, None, diag(n))).collect()).await;

    assert_eq!(events::count(&s).await.unwrap(), 250);
}

#[tokio::test]
async fn a_small_number_of_events_is_flushed_by_the_timer() {
    // Fewer than one batch: without the interval flush these would sit in memory indefinitely and
    // a crash would lose them entirely.
    let s = store().await;
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let writer = tokio::spawn(events::run_writer(s.clone(), rx));

    for n in 1..=3 {
        tx.send(envelope(n, None, diag(n))).await.unwrap();
    }

    // Wait past the batch interval without closing the channel.
    tokio::time::sleep(events::BATCH_INTERVAL * 5).await;
    assert_eq!(
        events::count(&s).await.unwrap(),
        3,
        "the timer should have committed a partial batch"
    );

    drop(tx);
    writer.await.unwrap();
}

#[tokio::test]
async fn since_returns_only_what_the_caller_has_not_seen() {
    // This is the gap-backfill contract: the frontend asks for everything after its last seq.
    let s = store().await;
    write_all(&s, (1..=20).map(|n| envelope(n, None, diag(n))).collect()).await;

    let tail = events::since(&s, Seq(15), 100).await.unwrap();
    assert_eq!(tail.len(), 5);
    assert_eq!(tail.first().unwrap().seq, Seq(16));
    assert_eq!(tail.last().unwrap().seq, Seq(20));
}

#[tokio::test]
async fn a_session_transcript_can_be_restored_after_a_restart() {
    let s = store().await;
    let mine = SessionId::new();
    let theirs = SessionId::new();

    let mut envelopes = Vec::new();
    for n in 1..=10 {
        let session = if n % 2 == 0 { mine } else { theirs };
        envelopes.push(envelope(n, Some(session), diag(n)));
    }
    write_all(&s, envelopes).await;

    let restored = events::for_session(&s, &mine.to_string(), 100)
        .await
        .unwrap();
    assert_eq!(restored.len(), 5);
    assert!(
        restored.iter().all(|e| e.session_id == Some(mine)),
        "another session's events must not leak into this transcript"
    );
}

#[tokio::test]
async fn event_payloads_round_trip_with_their_fields_intact() {
    // A transcript restored from disk must be the same one the user was watching.
    let s = store().await;
    let session = SessionId::new();

    write_all(
        &s,
        vec![
            envelope(
                1,
                Some(session),
                AgentEvent::ToolCall {
                    tool_use_id: "toolu_1".into(),
                    tool: "Write".into(),
                    input: serde_json::json!({ "file_path": "/tmp/x" }),
                },
            ),
            envelope(
                2,
                Some(session),
                AgentEvent::SessionExited {
                    reason: ExitReason::Crashed { code: Some(9) },
                },
            ),
        ],
    )
    .await;

    let read = events::since(&s, Seq(0), 10).await.unwrap();
    match &read[0].event {
        AgentEvent::ToolCall { tool, input, .. } => {
            assert_eq!(tool, "Write");
            assert_eq!(input["file_path"], "/tmp/x");
        }
        other => panic!("expected a tool call, got {other:?}"),
    }
    match &read[1].event {
        AgentEvent::SessionExited {
            reason: ExitReason::Crashed { code },
        } => assert_eq!(*code, Some(9)),
        other => panic!("expected a crash exit, got {other:?}"),
    }
}

#[tokio::test]
async fn max_seq_lets_a_restart_resume_numbering_without_colliding() {
    // Reusing a seq after restart would corrupt the frontend's gap detection permanently.
    let s = store().await;
    assert_eq!(events::max_seq(&s).await.unwrap(), Seq(0), "empty log");

    write_all(&s, (1..=7).map(|n| envelope(n, None, diag(n))).collect()).await;
    assert_eq!(events::max_seq(&s).await.unwrap(), Seq(7));
}

#[tokio::test]
async fn an_unreadable_row_is_skipped_rather_than_failing_the_whole_read() {
    // One event written by an older build must not make an entire transcript unrecoverable.
    let s = store().await;
    write_all(&s, vec![envelope(1, None, diag(1))]).await;

    sqlx::query(
        "INSERT INTO events (seq, at, kind, payload_json) VALUES (2, 0, 'mystery', 'not json')",
    )
    .execute(s.writer())
    .await
    .unwrap();

    write_all(&s, vec![envelope(3, None, diag(3))]).await;

    let read = events::since(&s, Seq(0), 100).await.unwrap();
    assert_eq!(read.len(), 2, "the good rows should still come back");
    assert_eq!(read[0].seq, Seq(1));
    assert_eq!(read[1].seq, Seq(3));
}

#[tokio::test]
async fn the_kind_column_is_populated_so_the_ui_can_filter_without_parsing_json() {
    let s = store().await;
    write_all(
        &s,
        vec![
            envelope(1, None, AgentEvent::TurnStarted),
            envelope(
                2,
                None,
                AgentEvent::PermissionRequest {
                    request_id: "r".into(),
                    tool: "Write".into(),
                    input: serde_json::Value::Null,
                    reason_type: None,
                    blocked_path: None,
                    suggestions: vec![],
                },
            ),
        ],
    )
    .await;

    let row: (String,) = sqlx::query_as("SELECT kind FROM events WHERE seq = 2")
        .fetch_one(s.reader())
        .await
        .unwrap();
    assert_eq!(row.0, "permission_request");
}

#[tokio::test]
async fn a_restart_continues_numbering_instead_of_colliding_with_the_old_log() {
    // The bug this exists for was invisible in tests and obvious the moment the app was run
    // twice: a bus that starts at 1 collides with every row the last launch wrote, the writer
    // logs and continues rather than crashing, and the durable log stops recording entirely.
    let s = store().await;
    write_all(&s, (1..=5).map(|n| envelope(n, None, diag(n))).collect()).await;

    // A fresh launch against the same database.
    let (bus, durable) = EventBus::new();
    bus.resume_from(events::max_seq(&s).await.unwrap());
    let writer = tokio::spawn(events::run_writer(s.clone(), durable));

    for n in 0..3 {
        bus.publish(Attribution::default(), diag(n)).await;
    }
    drop(bus);
    tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .expect("writer should finish")
        .unwrap();

    assert_eq!(
        events::count(&s).await.unwrap(),
        8,
        "the new events must be persisted, not rejected as duplicates"
    );
    assert_eq!(events::max_seq(&s).await.unwrap(), Seq(8));
}

#[tokio::test]
async fn events_published_through_the_bus_reach_the_store() {
    // The end-to-end path: nothing between publish and disk drops anything.
    let s = store().await;
    let (bus, durable) = EventBus::new();
    let writer = tokio::spawn(events::run_writer(s.clone(), durable));

    let session = SessionId::new();
    for n in 0..50 {
        bus.publish(
            Attribution {
                session_id: Some(session),
                ..Default::default()
            },
            diag(n),
        )
        .await;
    }

    drop(bus);
    tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .expect("writer should finish once the bus is dropped")
        .unwrap();

    assert_eq!(events::count(&s).await.unwrap(), 50);
}
