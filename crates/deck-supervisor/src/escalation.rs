//! Asking a human, in a form they can actually answer.
//!
//! An escalation used to be a counter. The run would increment it, park on `BlockedOnHuman`, and
//! the dashboard would say "waiting for a decision from you" — with no question, no options, and
//! nothing to click. Worse, the loop treated that phase as terminal and exited, so the promise
//! that it would "resume once you answer" was false twice over: there was nothing to answer, and
//! nothing left running to resume.
//!
//! So an escalation is a record with a question and a set of answers, and the answers are an
//! enum rather than free text. That is not ceremony. The whole design rests on the model never
//! being able to widen its own permissions, and a text box wired into the supervisor would be
//! exactly that hole — a human pasting "just skip the tests" has to be unrepresentable, not
//! merely discouraged.

use deck_core::domain::ids::TaskId;
use serde::{Deserialize, Serialize};

/// Why the run stopped to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationKind {
    /// The objective could not be decomposed. Nothing exists yet to retry.
    PlanningFailed,
    /// An agent could not be started — a worktree or spawn problem, not the agent's fault.
    DispatchFailed,
    /// A worker raised a blocker it cannot resolve itself.
    TaskBlocked,
    /// A task used up its attempts.
    AttemptsExhausted,
    /// Two agents produced work that will not merge.
    IntegrationConflict,
    /// Everything merged and the combined result fails its tests.
    IntegrationBroken,
    /// A limit was reached: iterations, cost, or wall clock.
    LimitReached,
}

impl EscalationKind {
    /// The answers that make sense, decided by code rather than offered by a model.
    ///
    /// Computed from the kind alone so the set cannot drift with prose. A planning failure has
    /// no task to retry, and a merge conflict cannot be fixed by trying the same merge again —
    /// offering those would be offering a button that does nothing.
    pub fn options(self, task: Option<TaskId>) -> Vec<EscalationOption> {
        let mut options = Vec::new();
        match self {
            EscalationKind::PlanningFailed => {
                options.push(EscalationOption::retry_planning());
            }
            EscalationKind::DispatchFailed
            | EscalationKind::TaskBlocked
            | EscalationKind::AttemptsExhausted => {
                if let Some(task) = task {
                    options.push(EscalationOption::retry_task(task));
                    options.push(EscalationOption::abandon_task(task));
                }
            }
            // Neither is fixed from in here. The operator resolves the conflict or the failure
            // in the integration worktree and then asks for another attempt; pretending we can
            // retry it unattended would just reproduce the same result.
            EscalationKind::IntegrationConflict | EscalationKind::IntegrationBroken => {
                options.push(EscalationOption::reintegrate());
            }
            EscalationKind::LimitReached => {}
        }
        // Always available, and always last: ending the run is the answer of last resort.
        options.push(EscalationOption::cancel_run());
        options
    }
}

/// A typed answer. Deliberately not free text — see the module note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum EscalationAnswer {
    /// Plan the objective again from scratch.
    RetryPlanning,
    /// Give a task another attempt, refunding the one it lost.
    RetryTask { task_id: TaskId },
    /// Accept that a task will not be done and let the rest of the run proceed.
    AbandonTask { task_id: TaskId },
    /// Merge and test the branches again, after the operator has changed something.
    Reintegrate,
    /// End the run.
    CancelRun,
}

/// One button, with the words that go on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscalationOption {
    pub label: String,
    /// What choosing it actually does, in the operator's terms rather than the code's.
    pub consequence: String,
    pub answer: EscalationAnswer,
    /// Marks an answer that discards work, so the UI can make it look like one.
    pub destructive: bool,
}

impl EscalationOption {
    fn retry_planning() -> Self {
        Self {
            label: "Plan again".into(),
            consequence: "Asks the supervisor to decompose the objective from scratch.".into(),
            answer: EscalationAnswer::RetryPlanning,
            destructive: false,
        }
    }

    fn retry_task(task_id: TaskId) -> Self {
        Self {
            label: "Try again".into(),
            consequence: "Gives this task another attempt with a fresh agent.".into(),
            answer: EscalationAnswer::RetryTask { task_id },
            destructive: false,
        }
    }

    fn abandon_task(task_id: TaskId) -> Self {
        Self {
            label: "Abandon this task".into(),
            consequence: "Leaves it unfinished. Anything depending on it cannot complete either."
                .into(),
            answer: EscalationAnswer::AbandonTask { task_id },
            destructive: true,
        }
    }

    fn reintegrate() -> Self {
        Self {
            label: "Merge again".into(),
            consequence: "Rebuilds the integration worktree from scratch and reruns the tests."
                .into(),
            answer: EscalationAnswer::Reintegrate,
            destructive: false,
        }
    }

    fn cancel_run() -> Self {
        Self {
            label: "End the run".into(),
            consequence: "Stops every agent. Worktrees and their changes are kept.".into(),
            answer: EscalationAnswer::CancelRun,
            destructive: true,
        }
    }
}

/// Something the run needs a person to decide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Escalation {
    pub id: String,
    pub kind: EscalationKind,
    pub task_id: Option<TaskId>,
    /// The decision, phrased as a question. Read straight into the UI.
    pub question: String,
    /// What went wrong, in the words of whatever produced it.
    pub detail: String,
    pub options: Vec<EscalationOption>,
    pub opened_at_iteration: u32,
}

impl Escalation {
    pub fn new(
        kind: EscalationKind,
        task_id: Option<TaskId>,
        question: impl Into<String>,
        detail: impl Into<String>,
        iteration: u32,
    ) -> Self {
        Self {
            // Random rather than sequential: an answer arrives asynchronously, and an id reused
            // across runs could apply yesterday's decision to today's question.
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            task_id,
            question: question.into(),
            detail: detail.into(),
            options: kind.options(task_id),
            opened_at_iteration: iteration,
        }
    }
}

/// Answers a human has given, waiting to be applied.
///
/// Drained by the driver rather than applied on arrival, for the same reason worker reports are:
/// a click lands at an arbitrary moment, and mutating the graph mid-stage would change it
/// underneath code already reading it.
pub trait AnswerQueue: Send + Sync {
    fn drain(&self) -> Vec<(String, EscalationAnswer)>;
}

/// Answers nothing. Used by tests and by any run with no operator attached.
pub struct NoAnswers;

impl AnswerQueue for NoAnswers {
    fn drain(&self) -> Vec<(String, EscalationAnswer)> {
        Vec::new()
    }
}
