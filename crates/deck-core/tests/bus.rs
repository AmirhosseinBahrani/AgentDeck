//! Event bus semantics. The important properties are about *loss*: which channel is allowed
//! to drop events and which must not.

use deck_core::bus::{Attribution, DeltaChannel, EventBus};
use deck_core::domain::event::AgentEvent;
use std::time::Duration;

fn diag(n: usize) -> AgentEvent {
    AgentEvent::Diagnostic {
        message: format!("event {n}"),
    }
}

#[tokio::test]
async fn seq_is_globally_monotonic_across_concurrent_publishers() {
    // Gap detection and SQLite backfill both key off seq, so two sessions publishing
    // concurrently must never be handed the same number.
    let (bus, mut durable) = EventBus::new();

    let mut handles = Vec::new();
    for _ in 0..8 {
        let bus = bus.clone();
        handles.push(tokio::spawn(async move {
            for n in 0..25 {
                bus.publish(Attribution::default(), diag(n)).await;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    let mut seqs = Vec::new();
    while let Ok(e) = durable.try_recv() {
        seqs.push(e.seq.0);
    }

    assert_eq!(seqs.len(), 200);
    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        200,
        "seq values were duplicated across publishers"
    );
    assert_eq!(
        seqs,
        {
            let mut s = seqs.clone();
            s.sort_unstable();
            s
        },
        "durable delivery must preserve seq order"
    );
}

#[tokio::test]
async fn durable_path_delivers_every_event() {
    let (bus, mut durable) = EventBus::new();
    for n in 0..500 {
        bus.publish(Attribution::default(), diag(n)).await;
    }

    let mut count = 0;
    while durable.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 500, "the audit log must not drop events");
}

#[tokio::test]
async fn publishing_succeeds_with_no_observers_attached() {
    // The UI is frequently not subscribed. That must never block persistence.
    let (bus, mut durable) = EventBus::new();
    bus.publish(Attribution::default(), diag(1)).await;
    assert!(durable.try_recv().is_ok());
}

#[tokio::test]
async fn a_lagging_observer_is_detectable_rather_than_silently_short() {
    let (bus, _durable) = EventBus::new();
    let mut observer = bus.subscribe();

    // Overrun the observer buffer without ever reading from it.
    for n in 0..5_000 {
        bus.publish(Attribution::default(), diag(n)).await;
    }

    let mut lagged = false;
    loop {
        match observer.try_recv() {
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                lagged = true;
                break;
            }
            Err(_) => break,
        }
    }
    assert!(
        lagged,
        "an overrun observer must surface Lagged so the consumer can backfill by seq \
         instead of silently missing events"
    );
}

#[tokio::test]
async fn durable_send_applies_backpressure_instead_of_growing_without_bound() {
    // Nothing drains the durable receiver here, so once the channel fills the publisher
    // must block. If it did not, a slow writer would let memory grow unboundedly.
    let (bus, _durable) = EventBus::new();

    let flood = tokio::spawn({
        let bus = bus.clone();
        async move {
            for n in 0..40_000 {
                bus.publish(Attribution::default(), diag(n)).await;
            }
        }
    });

    let blocked = tokio::time::timeout(Duration::from_millis(400), flood).await;
    assert!(
        blocked.is_err(),
        "publisher completed without a draining consumer, so the durable channel is \
         unbounded or lossy — both defeat the audit guarantee"
    );
}

#[tokio::test]
async fn deltas_are_dropped_when_nobody_is_watching_the_session() {
    // This is what makes inactive sessions free to stream: no subscriber, no work.
    let deltas = DeltaChannel::new();
    assert!(!deltas.has_subscribers());
    deltas.push("token".into()); // must not panic or block

    let mut sub = deltas.subscribe();
    assert!(deltas.has_subscribers());
    deltas.push("visible".into());
    assert_eq!(sub.try_recv().unwrap(), "visible");
}
