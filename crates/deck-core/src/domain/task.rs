//! Task lifecycle.
//!
//! The spec's central constraint is that the application owns state transitions and Claude only
//! reasons at bounded decision points. So every transition here is a pure function over a
//! legal-transition table: a model can *propose* that a task is done, but only this code can
//! move it, and only along an edge that exists.
//!
//! Counters are the other half of that. Attempts and review rounds are incremented here and
//! nowhere else, because they are what terminate the retry and rework loops. If a model could
//! influence them, the anti-infinite-loop guards would be advisory.

use super::ids::{AgentId, TaskId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Backlog,
    Queued,
    Assigned,
    Running,
    Blocked,
    Review,
    Completed,
    Failed,
    /// Stopped by the operator. Deliberately distinct from `Failed`: cancellation is user intent,
    /// not agent fault, so it must not consume a retry attempt or trigger reassignment.
    Cancelled,
}

impl TaskStatus {
    /// Terminal states are final. Reopening one would let a replan resurrect work whose
    /// worktree may already be gone.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }

    /// True while an agent may be holding a worktree and a session.
    pub fn is_active(self) -> bool {
        matches!(self, TaskStatus::Running | TaskStatus::Review)
    }
}

/// What happened, as observed by the application. Never a model's opinion.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskEvent {
    /// The supervisor decided this task is ready to be worked.
    Enqueued,
    Assigned {
        agent_id: AgentId,
    },
    /// A session started. Increments the attempt counter.
    Started,
    /// The worker called `claim_task_done` — the only legal completion path. A session that
    /// exits without it has failed, not finished.
    ClaimedDone,
    /// Deterministic verification and review both passed.
    ReviewPassed,
    /// Review rejected the work. Increments the review-round counter.
    ReviewFailed {
        reason: String,
    },
    Blocked {
        reason: String,
    },
    /// A blocker was resolved, by a human or by a dependency completing.
    Unblocked,
    /// The agent or its session failed.
    Failed {
        reason: String,
    },
    /// Explicit operator stop.
    Cancelled,
    /// A dependency failed permanently, so this task cannot proceed.
    DependencyFailed {
        blocking: TaskId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskLimits {
    pub max_attempts: u32,
    pub max_review_rounds: u32,
}

impl Default for TaskLimits {
    fn default() -> Self {
        // Deliberately small. These are the guards that stop a review/rework loop burning the
        // rate limit; generous values would defeat the purpose.
        Self {
            max_attempts: 3,
            max_review_rounds: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskState {
    pub id: TaskId,
    pub status: TaskStatus,
    pub assignee: Option<AgentId>,
    pub attempts: u32,
    pub review_rounds: u32,
    pub limits: TaskLimits,
    pub failure_reason: Option<String>,
    pub blocking_task: Option<TaskId>,
    /// Set when the task cannot be considered complete without succeeding.
    pub objective_gate: bool,
}

impl TaskState {
    pub fn new(id: TaskId) -> Self {
        Self {
            id,
            status: TaskStatus::Backlog,
            assignee: None,
            attempts: 0,
            review_rounds: 0,
            limits: TaskLimits::default(),
            failure_reason: None,
            blocking_task: None,
            objective_gate: false,
        }
    }

    pub fn attempts_remaining(&self) -> u32 {
        self.limits.max_attempts.saturating_sub(self.attempts)
    }

    pub fn review_rounds_remaining(&self) -> u32 {
        self.limits
            .max_review_rounds
            .saturating_sub(self.review_rounds)
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TransitionError {
    #[error("{event:?} is not legal from {from:?}")]
    Illegal { from: TaskStatus, event: TaskEvent },
    #[error("task is already terminal ({0:?}) and cannot be reopened")]
    Terminal(TaskStatus),
    #[error("cannot start an unassigned task")]
    Unassigned,
}

/// Applies an event. Returns the new state, or refuses.
///
/// Total and side-effect free, so the supervisor's decisions can be replayed against it as a
/// regression corpus.
pub fn apply(state: &TaskState, event: TaskEvent) -> Result<TaskState, TransitionError> {
    use TaskEvent as E;
    use TaskStatus as S;

    // Cancellation is the one thing allowed from any non-terminal state: the operator must
    // always be able to stop an agent.
    if let E::Cancelled = event {
        if state.status.is_terminal() {
            return Err(TransitionError::Terminal(state.status));
        }
        return Ok(TaskState {
            status: S::Cancelled,
            // Note: attempts is NOT incremented. Cancelling is user intent, so it must not
            // consume a retry or make the task look like a repeat failure.
            ..state.clone()
        });
    }

    if state.status.is_terminal() {
        return Err(TransitionError::Terminal(state.status));
    }

    let next = match (state.status, &event) {
        (S::Backlog, E::Enqueued) => TaskState {
            status: S::Queued,
            ..state.clone()
        },

        (S::Queued | S::Backlog, E::Assigned { agent_id }) => TaskState {
            status: S::Assigned,
            assignee: Some(*agent_id),
            ..state.clone()
        },

        // Reassignment while assigned or blocked is legal; the failure handler uses it.
        (S::Assigned | S::Blocked, E::Assigned { agent_id }) => TaskState {
            status: S::Assigned,
            assignee: Some(*agent_id),
            blocking_task: None,
            ..state.clone()
        },

        (S::Assigned, E::Started) => {
            if state.assignee.is_none() {
                return Err(TransitionError::Unassigned);
            }
            TaskState {
                status: S::Running,
                // The only place attempts increases. A dispatch is an attempt whether or not it
                // ends in failure, which is what makes the retry cap meaningful.
                attempts: state.attempts + 1,
                ..state.clone()
            }
        }

        (S::Running, E::ClaimedDone) => TaskState {
            status: S::Review,
            ..state.clone()
        },

        (S::Review, E::ReviewPassed) => TaskState {
            status: S::Completed,
            ..state.clone()
        },

        (S::Review, E::ReviewFailed { reason }) => {
            let review_rounds = state.review_rounds + 1;
            // Out of rework rounds is a permanent failure, not another loop. Escalation happens
            // above this layer; here the task simply stops.
            let status = if review_rounds >= state.limits.max_review_rounds {
                S::Failed
            } else {
                S::Assigned
            };
            TaskState {
                status,
                review_rounds,
                failure_reason: (status == S::Failed).then(|| reason.clone()),
                ..state.clone()
            }
        }

        // Queued is included deliberately: the supervisor can escalate an ambiguity about a
        // task before anyone is dispatched to it, and that task is blocked, not merely waiting
        // its turn.
        (S::Queued | S::Running | S::Assigned | S::Review, E::Blocked { reason }) => TaskState {
            status: S::Blocked,
            failure_reason: Some(reason.clone()),
            ..state.clone()
        },

        (S::Blocked, E::Unblocked) => TaskState {
            // Back to the queue rather than straight to running: capacity and resource locks
            // must be re-acquired, and the previous assignee may no longer be free.
            status: S::Queued,
            failure_reason: None,
            blocking_task: None,
            ..state.clone()
        },

        (S::Running | S::Assigned | S::Review, E::Failed { reason }) => {
            // Attempts were already counted at Started, so a failure just decides whether any
            // remain.
            let status = if state.attempts >= state.limits.max_attempts {
                S::Failed
            } else {
                S::Queued
            };
            TaskState {
                status,
                failure_reason: Some(reason.clone()),
                ..state.clone()
            }
        }

        (_, E::DependencyFailed { blocking }) => TaskState {
            // Blocked, not failed: a fix task may still unblock this one, and cancelling here
            // would throw away work that is only waiting.
            status: S::Blocked,
            blocking_task: Some(*blocking),
            failure_reason: Some(format!("blocked by failed task {blocking}")),
            ..state.clone()
        },

        (from, event) => {
            return Err(TransitionError::Illegal {
                from,
                event: event.clone(),
            })
        }
    };

    Ok(next)
}

/// Whether a task is eligible for dispatch. Dependency readiness and capacity are checked by
/// the scheduler; this is only the task's own contribution.
pub fn is_dispatchable(state: &TaskState) -> bool {
    matches!(state.status, TaskStatus::Assigned) && state.assignee.is_some()
}
