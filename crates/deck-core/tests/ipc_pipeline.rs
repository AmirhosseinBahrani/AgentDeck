//! End-to-end check of the path the UI actually depends on:
//! MockRuntime -> EventBus -> Coalescer -> batches.
//!
//! The webview is not involved, so this runs in CI and pins the batching guarantees the
//! frontend's gap detection and delta rendering are built on.

use deck_core::bus::{Attribution, EventBus};
use deck_core::domain::event::AgentEvent;
use deck_core::domain::ids::{Seq, SessionId};
use deck_core::ipc::{is_critical, Coalescer, MAX_BATCH};
use deck_core::runtime::mock::{MockRuntime, Speed};
use std::path::Path;
use std::sync::Arc;

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

/// Feeds everything the bus produced through a coalescer, mimicking the Tauri task.
fn batch_all(
    mut rx: tokio::sync::mpsc::Receiver<deck_core::domain::event::EventEnvelope>,
    watched: Vec<SessionId>,
) -> Vec<deck_core::ipc::EventBatch> {
    let mut coalescer = Coalescer::new();
    coalescer.set_watched(watched);
    let mut batches = Vec::new();

    while let Ok(envelope) = rx.try_recv() {
        if is_critical(&envelope.event) {
            if let Some(b) = coalescer.flush() {
                batches.push(b);
            }
            let seq = envelope.seq;
            batches.push(deck_core::ipc::EventBatch {
                first_seq: Some(seq),
                last_seq: Some(seq),
                events: vec![envelope],
                deltas: Vec::new(),
            });
            continue;
        }
        coalescer.push_event(envelope);
        if coalescer.should_flush() {
            if let Some(b) = coalescer.flush() {
                batches.push(b);
            }
        }
    }
    if let Some(b) = coalescer.flush() {
        batches.push(b);
    }
    batches
}

#[tokio::test]
async fn a_replayed_session_reaches_the_ui_as_ordered_gapless_batches() {
    let (bus, durable) = EventBus::new();
    let mock = MockRuntime::new(bus, Speed::Immediate);
    mock.register_fixture_dir(fixtures()).expect("fixtures");

    let session = SessionId::new();
    mock.replay(
        "probe1.ndjson",
        Attribution {
            session_id: Some(session),
            ..Default::default()
        },
    )
    .await
    .expect("replay");

    let batches = batch_all(durable, vec![session]);
    assert!(!batches.is_empty(), "expected at least one batch");

    // Concatenating batches must reproduce the original stream exactly: same order, no
    // duplicates, no holes. Everything the frontend does assumes this.
    let seqs: Vec<u64> = batches
        .iter()
        .flat_map(|b| b.events.iter().map(|e| e.seq.0))
        .collect();

    let mut expected: Vec<u64> = seqs.clone();
    expected.sort_unstable();
    assert_eq!(seqs, expected, "batching reordered events");
    expected.dedup();
    assert_eq!(seqs.len(), expected.len(), "batching duplicated events");

    for window in seqs.windows(2) {
        assert_eq!(
            window[1],
            window[0] + 1,
            "gap between {} and {} would trigger a spurious backfill",
            window[0],
            window[1]
        );
    }

    // Each batch's advertised range must match its contents, since the frontend trusts the
    // range rather than re-deriving it.
    for b in &batches {
        assert_eq!(b.first_seq, b.events.first().map(|e| e.seq));
        assert_eq!(b.last_seq, b.events.last().map(|e| e.seq));
    }
}

#[tokio::test]
async fn permission_requests_arrive_after_the_tool_call_that_caused_them() {
    // Critical events bypass batching, so ordering has to be preserved explicitly. Getting
    // this wrong would show the user a permission prompt before the action it refers to.
    let (bus, durable) = EventBus::new();
    let session = SessionId::new();
    let attribution = Attribution {
        session_id: Some(session),
        ..Default::default()
    };

    bus.publish(
        attribution,
        AgentEvent::ToolCall {
            tool_use_id: "t1".into(),
            tool: "Write".into(),
            input: serde_json::json!({"file_path": "/etc/hosts"}),
        },
    )
    .await;
    bus.publish(
        attribution,
        AgentEvent::PermissionRequest {
            request_id: "r1".into(),
            tool: "Write".into(),
            input: serde_json::json!({"file_path": "/etc/hosts"}),
            reason_type: Some("workingDir".into()),
            blocked_path: Some("/etc/hosts".into()),
            suggestions: vec![],
        },
    )
    .await;

    let batches = batch_all(durable, vec![session]);
    let flat: Vec<&AgentEvent> = batches
        .iter()
        .flat_map(|b| b.events.iter().map(|e| &e.event))
        .collect();

    let call_at = flat
        .iter()
        .position(|e| matches!(e, AgentEvent::ToolCall { .. }))
        .expect("tool call");
    let perm_at = flat
        .iter()
        .position(|e| matches!(e, AgentEvent::PermissionRequest { .. }))
        .expect("permission request");

    assert!(
        call_at < perm_at,
        "the permission prompt overtook its own tool call"
    );
}

#[tokio::test]
async fn concurrent_sessions_stay_attributed_and_only_watched_ones_stream_text() {
    let (bus, durable) = EventBus::new();
    let mock = Arc::new(MockRuntime::new(bus.clone(), Speed::Immediate));
    mock.register_fixture_dir(fixtures()).expect("fixtures");

    let watched = SessionId::new();
    let hidden = SessionId::new();

    for id in [watched, hidden] {
        mock.replay(
            "probe1.ndjson",
            Attribution {
                session_id: Some(id),
                ..Default::default()
            },
        )
        .await
        .expect("replay");
    }

    // Simulate the UI displaying only one of the two.
    let mut coalescer = Coalescer::new();
    coalescer.set_watched(vec![watched]);
    coalescer.push_delta(watched, "visible text");
    coalescer.push_delta(hidden, "should never cross the bridge");

    let mut rx = durable;
    let mut per_session = std::collections::HashMap::new();
    while let Ok(env) = rx.try_recv() {
        *per_session.entry(env.session_id).or_insert(0usize) += 1;
        coalescer.push_event(env);
    }

    let batch = coalescer.flush().expect("batch");

    assert_eq!(
        per_session.len(),
        2,
        "both sessions' events must be recorded, regardless of visibility"
    );
    assert_eq!(
        batch.deltas.len(),
        1,
        "only the watched session streams text"
    );
    assert_eq!(batch.deltas[0].session_id, watched);
}

#[tokio::test]
async fn a_burst_larger_than_one_batch_is_split_without_loss() {
    let (bus, durable) = EventBus::new();
    let session = SessionId::new();
    let attribution = Attribution {
        session_id: Some(session),
        ..Default::default()
    };

    let total = MAX_BATCH * 2 + 7;
    for _ in 0..total {
        bus.publish(attribution, AgentEvent::TurnStarted).await;
    }

    let batches = batch_all(durable, vec![session]);
    let delivered: usize = batches.iter().map(|b| b.events.len()).sum();

    assert!(batches.len() >= 3, "a burst should split across batches");
    assert_eq!(delivered, total, "splitting must not drop events");
    assert_eq!(batches[0].first_seq, Some(Seq(1)));
}
