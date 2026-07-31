//! Coalescing layer between the event bus and whatever transport reaches the UI.
//!
//! Emitting one IPC message per event does not survive contact with reality: several agents
//! streaming token deltas concurrently produce thousands of messages per second, and the
//! bridge becomes the bottleneck. So events are batched by time and count, text deltas are
//! concatenated per block before crossing, and each batch carries a seq range so the
//! frontend can detect a gap and backfill from SQLite rather than silently missing events.
//!
//! Kept in `deck-core` rather than the Tauri crate so it is testable without a webview.

use crate::domain::event::{AgentEvent, EventEnvelope};
use crate::domain::ids::{Seq, SessionId};
use serde::Serialize;
use std::collections::HashMap;
use std::time::Duration;

/// Flush interval. ~60fps: batching more finely gains nothing because the UI cannot paint
/// faster, and batching more coarsely makes streaming text feel laggy.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// Flush early if a batch reaches this many events, so a burst does not wait out the timer.
pub const MAX_BATCH: usize = 256;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EventBatch {
    pub events: Vec<EventEnvelope>,
    /// Text accumulated per session since the last batch, already concatenated.
    pub deltas: Vec<DeltaChunk>,
    /// Inclusive seq range covered. A frontend whose last seen seq is below `first_seq - 1`
    /// has missed events and must call `get_events_since`.
    pub first_seq: Option<Seq>,
    pub last_seq: Option<Seq>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeltaChunk {
    pub session_id: SessionId,
    pub text: String,
}

/// Accumulates events and emits batches. Transport-agnostic and synchronous, so the batching
/// policy can be unit-tested without a runtime or a webview.
#[derive(Debug, Default)]
pub struct Coalescer {
    events: Vec<EventEnvelope>,
    /// Per-session text accumulator. Concatenating here rather than sending one message per
    /// token is the single largest saving on the bridge.
    deltas: HashMap<SessionId, String>,
    /// Sessions the UI is currently displaying. Deltas for anything else are discarded:
    /// nobody can see them, and they are the bulk of the traffic.
    watched: Vec<SessionId>,
}

impl Coalescer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_watched(&mut self, sessions: Vec<SessionId>) {
        // Drop buffered text for sessions that just stopped being watched, so switching tabs
        // does not dump a backlog of now-irrelevant tokens into the next batch.
        self.deltas.retain(|id, _| sessions.contains(id));
        self.watched = sessions;
    }

    pub fn is_watched(&self, session: SessionId) -> bool {
        self.watched.contains(&session)
    }

    pub fn push_event(&mut self, envelope: EventEnvelope) {
        self.events.push(envelope);
    }

    /// Buffers a token delta. Ignored for unwatched sessions.
    pub fn push_delta(&mut self, session: SessionId, text: &str) {
        if !self.is_watched(session) {
            return;
        }
        self.deltas.entry(session).or_default().push_str(text);
    }

    pub fn should_flush(&self) -> bool {
        self.events.len() >= MAX_BATCH
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.deltas.is_empty()
    }

    /// Takes everything buffered. Returns `None` when there is nothing to send, so an idle
    /// app produces no IPC traffic at all.
    pub fn flush(&mut self) -> Option<EventBatch> {
        if self.is_empty() {
            return None;
        }
        let events = std::mem::take(&mut self.events);
        let first_seq = events.first().map(|e| e.seq);
        let last_seq = events.last().map(|e| e.seq);
        let deltas = std::mem::take(&mut self.deltas)
            .into_iter()
            .map(|(session_id, text)| DeltaChunk { session_id, text })
            .collect();

        Some(EventBatch {
            events,
            deltas,
            first_seq,
            last_seq,
        })
    }
}

/// Events that must not wait for the next batch: anything the user has to act on, or that
/// changes whether an agent is still running. Batching these would add latency exactly where
/// it is most visible.
pub fn is_critical(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::PermissionRequest { .. }
            | AgentEvent::SessionExited { .. }
            | AgentEvent::RateLimited { .. }
            | AgentEvent::Diagnostic { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ids::{AgentId, TaskId};

    fn env(seq: u64, session: SessionId) -> EventEnvelope {
        EventEnvelope {
            seq: Seq(seq),
            at_ms: 0,
            session_id: Some(session),
            agent_id: None::<AgentId>,
            task_id: None::<TaskId>,
            event: AgentEvent::TurnStarted,
        }
    }

    #[test]
    fn idle_coalescer_produces_no_batch() {
        // An idle app must generate zero IPC traffic, not empty heartbeat messages.
        let mut c = Coalescer::new();
        assert!(c.flush().is_none());
    }

    #[test]
    fn batch_carries_the_seq_range_it_covers() {
        let s = SessionId::new();
        let mut c = Coalescer::new();
        for seq in 5..=9 {
            c.push_event(env(seq, s));
        }
        let batch = c.flush().expect("batch");
        assert_eq!(batch.first_seq, Some(Seq(5)));
        assert_eq!(batch.last_seq, Some(Seq(9)));
        assert_eq!(batch.events.len(), 5);
    }

    #[test]
    fn consecutive_batches_leave_no_seq_gap() {
        // The frontend treats a gap as "I missed events, refetch". A spurious gap would cause
        // pointless backfills; a missed gap would hide real loss.
        let s = SessionId::new();
        let mut c = Coalescer::new();
        c.push_event(env(1, s));
        c.push_event(env(2, s));
        let first = c.flush().unwrap();
        c.push_event(env(3, s));
        let second = c.flush().unwrap();

        assert_eq!(first.last_seq.unwrap().0 + 1, second.first_seq.unwrap().0);
    }

    #[test]
    fn deltas_are_concatenated_per_session_not_sent_per_token() {
        let a = SessionId::new();
        let b = SessionId::new();
        let mut c = Coalescer::new();
        c.set_watched(vec![a, b]);

        for part in ["He", "ll", "o"] {
            c.push_delta(a, part);
        }
        c.push_delta(b, "hi");

        let batch = c.flush().unwrap();
        assert_eq!(
            batch.deltas.len(),
            2,
            "one chunk per session, not per token"
        );
        let text_for = |s: SessionId| {
            batch
                .deltas
                .iter()
                .find(|d| d.session_id == s)
                .map(|d| d.text.clone())
                .unwrap()
        };
        assert_eq!(text_for(a), "Hello");
        assert_eq!(text_for(b), "hi");
    }

    #[test]
    fn deltas_for_unwatched_sessions_are_discarded() {
        // This is what makes many concurrent agents affordable: only the visible session's
        // token stream crosses the bridge.
        let watched = SessionId::new();
        let hidden = SessionId::new();
        let mut c = Coalescer::new();
        c.set_watched(vec![watched]);

        c.push_delta(hidden, "invisible");
        assert!(c.is_empty(), "unwatched deltas must not buffer");

        c.push_delta(watched, "visible");
        let batch = c.flush().unwrap();
        assert_eq!(batch.deltas.len(), 1);
        assert_eq!(batch.deltas[0].session_id, watched);
    }

    #[test]
    fn events_are_never_dropped_for_unwatched_sessions() {
        // Deltas are cosmetic, but real events drive task and agent state, so they must flow
        // regardless of what the user happens to be looking at.
        let hidden = SessionId::new();
        let mut c = Coalescer::new();
        c.set_watched(vec![]);
        c.push_event(env(1, hidden));
        assert_eq!(c.flush().unwrap().events.len(), 1);
    }

    #[test]
    fn unwatching_a_session_drops_its_buffered_text() {
        let a = SessionId::new();
        let mut c = Coalescer::new();
        c.set_watched(vec![a]);
        c.push_delta(a, "stale");
        c.set_watched(vec![]);
        assert!(
            c.flush().is_none(),
            "switching away must not later dump a backlog of irrelevant tokens"
        );
    }

    #[test]
    fn a_burst_flushes_on_count_without_waiting_for_the_timer() {
        let s = SessionId::new();
        let mut c = Coalescer::new();
        for seq in 0..MAX_BATCH as u64 {
            c.push_event(env(seq, s));
        }
        assert!(c.should_flush());
    }

    #[test]
    fn permission_requests_and_exits_bypass_batching() {
        // Latency on these is directly user-visible: a blocking prompt or a dead agent.
        assert!(is_critical(&AgentEvent::PermissionRequest {
            request_id: "r".into(),
            tool: "Write".into(),
            input: serde_json::Value::Null,
            reason_type: None,
            blocked_path: None,
            suggestions: vec![],
        }));
        assert!(is_critical(&AgentEvent::SessionExited {
            reason: crate::domain::event::ExitReason::Clean
        }));
        assert!(!is_critical(&AgentEvent::TurnStarted));
    }
}
