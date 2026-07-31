//! Exercises the fixture-replay runtime, including the concurrent-load case that stands in
//! for several agents streaming at once.

use deck_core::bus::{Attribution, EventBus};
use deck_core::domain::event::AgentEvent;
use deck_core::runtime::mock::{MockRuntime, Speed};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fixtures() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

#[tokio::test]
async fn replays_a_fixture_through_the_same_translator_as_the_live_runtime() {
    let (bus, mut durable) = EventBus::new();
    let mock = MockRuntime::new(bus, Speed::Immediate);
    let loaded = mock
        .register_fixture_dir(fixtures())
        .expect("load fixtures");
    assert!(loaded >= 5, "expected the captured fixtures to be present");

    let (session, _) = mock
        .replay("probe1.ndjson", Attribution::default())
        .await
        .expect("probe1 should be registered");

    let mut events = Vec::new();
    while let Ok(env) = durable.try_recv() {
        assert_eq!(env.session_id, Some(session.session_id));
        events.push(env.event);
    }

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionReady { .. })),
        "replay should produce the same handshake a live session would"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { .. })),
        "replay should surface tool calls"
    );
    assert!(
        matches!(events.last(), Some(AgentEvent::SessionExited { .. })),
        "a replayed session must terminate like a real one"
    );
}

#[tokio::test]
async fn inline_scripts_let_a_test_dictate_exact_output() {
    // The supervisor needs to test specific decision shapes, which recorded sessions cannot
    // provide on demand.
    let (bus, mut durable) = EventBus::new();
    let mock = MockRuntime::new(bus, Speed::Immediate);
    mock.register(
        "scripted",
        r#"{"type":"system","subtype":"init","cwd":"/w","tools":[]}
{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.5}"#,
    );

    mock.replay("scripted", Attribution::default())
        .await
        .expect("scripted replay");

    let mut costs = Vec::new();
    while let Ok(env) = durable.try_recv() {
        if let AgentEvent::TurnComplete { cost_usd, .. } = env.event {
            costs.extend(cost_usd);
        }
    }
    assert_eq!(costs, vec![0.5]);
}

#[tokio::test]
async fn unknown_script_names_are_reported_rather_than_panicking() {
    let (bus, _durable) = EventBus::new();
    let mock = MockRuntime::new(bus, Speed::Immediate);
    assert!(mock
        .replay("nope.ndjson", Attribution::default())
        .await
        .is_none());
}

#[tokio::test]
async fn concurrent_replay_delivers_every_event_from_every_session() {
    // Stands in for the real stress case: several agents streaming simultaneously. Events
    // must not be interleaved into the wrong session or dropped.
    let (bus, mut durable) = EventBus::new();
    let drained = tokio::spawn(async move {
        let mut per_session: std::collections::HashMap<_, usize> = Default::default();
        while let Some(env) = durable.recv().await {
            *per_session.entry(env.session_id).or_default() += 1;
        }
        per_session
    });

    let mock = Arc::new(MockRuntime::new(bus, Speed::Immediate));
    mock.register_fixture_dir(fixtures())
        .expect("load fixtures");

    const SESSIONS: usize = 12;
    let started = Instant::now();
    let ids = mock.replay_concurrent("probe1.ndjson", SESSIONS).await;
    let elapsed = started.elapsed();

    assert_eq!(ids.len(), SESSIONS);

    // Drop the runtime so the bus sender closes and the drain task finishes.
    drop(mock);
    let per_session = tokio::time::timeout(Duration::from_secs(10), drained)
        .await
        .expect("drain should finish once the bus closes")
        .expect("drain task");

    assert_eq!(
        per_session.len(),
        SESSIONS,
        "each replayed session must be attributed distinctly, not merged"
    );
    for (session, count) in &per_session {
        assert!(
            *count > 1,
            "session {session:?} produced only {count} events, so replay was truncated"
        );
    }

    // Not a hard performance gate, just a guard against accidental serialization (e.g. a
    // global lock held across an await) making the harness useless for load work.
    assert!(
        elapsed < Duration::from_secs(20),
        "concurrent replay took {elapsed:?}, which suggests it is not actually concurrent"
    );
}
