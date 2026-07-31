//! The supervisor is a deterministic controller around Claude, not a long-lived prompt.
//!
//! Its memory is the database: every decision is a one-shot `claude -p --json-schema`
//! call with a code-assembled input projection, and every model output passes a validation
//! ladder before it is allowed to mutate state. State transitions, retry counting,
//! dependency readiness, concurrency limits and completion detection are pure code.
//!
//! Implemented in M6; this crate exists from M0 so the compile boundary is real.

pub mod contract;
pub mod graph;

/// The stage pipeline of one supervisor iteration. `Sweep` runs every tick and is free;
/// only a dirty run advances through the stages, so an idle run costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Observe,
    IngestReports,
    Reap,
    Plan,
    Assign,
    Dispatch,
    Verify,
    Judge,
    Adjudicate,
    Failures,
    Escalate,
    CompletionCheck,
    Commit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Planning,
    Dispatching,
    Monitoring,
    Reviewing,
    Replanning,
    BlockedOnHuman,
    Completed,
    Failed,
    Cancelled,
}
