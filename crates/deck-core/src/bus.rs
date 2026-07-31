//! Event distribution.
//!
//! Three channels with deliberately different semantics, because the consumers have
//! different tolerance for loss:
//!
//! - **Durable** — bounded `mpsc`, awaited. If the writer falls behind, publishers block,
//!   which backpressures the agent. Slowing an agent is preferable to losing its audit log.
//! - **Observer** — `broadcast`, lossy. UI and supervisor tolerate gaps because `seq` makes
//!   them detectable and SQLite can backfill.
//! - **Deltas** — per-session `broadcast` for token partials. Never persisted, never on the
//!   durable path; these are the highest-volume and least valuable events.

use crate::domain::event::AgentEvent;
use crate::domain::event::EventEnvelope;
use crate::domain::ids::{AgentId, Seq, SessionId, TaskId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

const DURABLE_CAPACITY: usize = 16_384;
const OBSERVER_CAPACITY: usize = 4_096;

/// What a lagging observer receives so it knows to refetch rather than silently skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resync {
    pub missed: u64,
}

#[derive(Debug, Clone)]
pub enum Observed {
    Event(EventEnvelope),
    /// The observer fell behind and lost events. Backfill from `events` by `seq`.
    Resync(Resync),
}

/// Attribution attached to every event a session publishes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Attribution {
    pub session_id: Option<SessionId>,
    pub agent_id: Option<AgentId>,
    pub task_id: Option<TaskId>,
}

pub struct EventBus {
    /// One process-wide counter. Global monotonic ordering is what makes gap detection and
    /// backfill possible at all; per-session counters could not be merged coherently.
    seq: AtomicU64,
    durable_tx: mpsc::Sender<EventEnvelope>,
    observer_tx: broadcast::Sender<EventEnvelope>,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl EventBus {
    /// Returns the bus and the durable receiver, which the persistence task owns.
    pub fn new() -> (Arc<Self>, mpsc::Receiver<EventEnvelope>) {
        Self::with_clock(Box::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or_default()
        }))
    }

    pub fn with_clock(
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> (Arc<Self>, mpsc::Receiver<EventEnvelope>) {
        let (durable_tx, durable_rx) = mpsc::channel(DURABLE_CAPACITY);
        let (observer_tx, _) = broadcast::channel(OBSERVER_CAPACITY);
        let bus = Arc::new(Self {
            seq: AtomicU64::new(0),
            durable_tx,
            observer_tx,
            clock,
        });
        (bus, durable_rx)
    }

    pub fn next_seq(&self) -> Seq {
        Seq(self.seq.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Publishes to both the durable and observer paths.
    ///
    /// Awaits the durable send, so a stalled writer throttles the producer rather than
    /// dropping history. The observer send is fire-and-forget: no subscribers, or a full
    /// buffer, must never block persistence.
    pub async fn publish(&self, attribution: Attribution, event: AgentEvent) -> Seq {
        let envelope = EventEnvelope {
            seq: self.next_seq(),
            at_ms: (self.clock)(),
            session_id: attribution.session_id,
            agent_id: attribution.agent_id,
            task_id: attribution.task_id,
            event,
        };
        let seq = envelope.seq;

        let _ = self.observer_tx.send(envelope.clone());

        if self.durable_tx.send(envelope).await.is_err() {
            // The persistence task is gone, which happens during shutdown. Losing an event
            // then is acceptable; failing the caller is not.
            tracing::debug!("durable event sink closed, dropping event");
        }
        seq
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.observer_tx.subscribe()
    }

    /// Number of events queued for persistence. Exposed so a load test can assert that
    /// backpressure engages rather than the queue growing without bound.
    pub fn durable_queue_depth(&self) -> usize {
        DURABLE_CAPACITY - self.durable_tx.capacity()
    }
}

/// Per-session channel for token deltas. Separate from the bus so that partial-message
/// traffic cannot crowd out real events or reach persistence.
#[derive(Debug, Clone)]
pub struct DeltaChannel {
    tx: broadcast::Sender<String>,
}

impl DeltaChannel {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    /// Dropped silently when nobody is watching this session, which is the common case and
    /// the reason inactive sessions cost nothing to stream.
    pub fn push(&self, chunk: String) {
        let _ = self.tx.send(chunk);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    pub fn has_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }
}

impl Default for DeltaChannel {
    fn default() -> Self {
        Self::new()
    }
}
