//! The supervisor control loop.
//!
//! Two operations, and separating them is the core cost-safety mechanism:
//!
//! - **Sweep** runs on every tick, is pure, and costs nothing. It recomputes stall counters,
//!   budget state and `progress_possible`, and may set a dirty flag.
//! - **Iterate** runs only when dirty, and is the only thing that may call Claude.
//!
//! So an idle run is free. Without that split a tick-driven loop would spend money to discover
//! that nothing had happened, which at a 2-second tick is most of what it would ever do.
//!
//! Crash-resume rests on stage receipts: each stage's receipt is written in the same transaction
//! as its mutations, so a restart skips stages that already committed rather than repeating side
//! effects.

use crate::decision::{DecidedBy, DecisionRecord};
use crate::graph::TaskGraph;
use deck_core::domain::task::TaskStatus;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl Stage {
    /// The fixed order of one iteration.
    pub const PIPELINE: [Stage; 13] = [
        Stage::Observe,
        Stage::IngestReports,
        Stage::Reap,
        Stage::Plan,
        Stage::Assign,
        Stage::Dispatch,
        Stage::Verify,
        Stage::Judge,
        Stage::Adjudicate,
        Stage::Failures,
        Stage::Escalate,
        Stage::CompletionCheck,
        Stage::Commit,
    ];

    /// Whether this stage may consult Claude. Everything else is pure code, and that division is
    /// the design's central claim.
    pub fn may_call_model(self) -> bool {
        matches!(
            self,
            Stage::Plan | Stage::Assign | Stage::Judge | Stage::Failures | Stage::Escalate
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl RunPhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunPhase::Completed | RunPhase::Failed | RunPhase::Cancelled
        )
    }
}

/// Hard ceilings on a run. Without these a pathological objective could iterate indefinitely; the
/// budget cap in particular is what stops a runaway loop from consuming the account's limit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RunLimits {
    pub max_iterations: u32,
    pub max_cost_usd: f64,
    pub max_replans: u32,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_iterations: 200,
            max_cost_usd: 25.0,
            max_replans: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub phase: RunPhase,
    pub iteration: u32,
    pub replans: u32,
    /// Summed from per-turn `result.total_cost_usd`, which the CLI reports per turn rather than
    /// cumulatively.
    pub spent_usd: f64,
    pub open_escalations: usize,
    /// Whether every completed branch has been merged and tested together.
    ///
    /// On the run state rather than beside it because the sweep is what declares a run complete,
    /// and a completion predicate that could not see this would declare success over a set of
    /// branches nobody had ever combined.
    pub integrated: bool,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            phase: RunPhase::Planning,
            iteration: 0,
            replans: 0,
            spent_usd: 0.0,
            open_escalations: 0,
            integrated: false,
        }
    }
}

/// Why the loop woke up. A tick is free; everything else marks the run dirty.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    Tick,
    ReportReceived,
    SessionExited,
    TaskClaimedDone,
    HumanAnswered,
    CancelRequested,
}

impl Trigger {
    /// Whether this trigger justifies an iteration on its own.
    ///
    /// A tick does not: it only runs the free sweep, and the sweep decides whether anything
    /// actually needs doing.
    pub fn marks_dirty(&self) -> bool {
        !matches!(self, Trigger::Tick)
    }
}

/// Outcome of the pure sweep. Never calls a model, never mutates anything.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepOutcome {
    pub should_iterate: bool,
    pub terminal: Option<RunPhase>,
    pub notes: Vec<String>,
}

/// Decides whether an iteration is warranted, and whether the run should stop.
///
/// Ordering matters: caps and deadlock are checked before "is there work", so a run that has
/// exhausted its budget stops rather than doing one more expensive iteration first.
pub fn sweep(state: &RunState, graph: &TaskGraph, limits: RunLimits, dirty: bool) -> SweepOutcome {
    let mut notes = Vec::new();

    if state.phase.is_terminal() {
        return SweepOutcome {
            should_iterate: false,
            terminal: Some(state.phase),
            notes,
        };
    }

    if state.iteration >= limits.max_iterations {
        notes.push(format!(
            "iteration cap reached ({}); stopping rather than looping",
            limits.max_iterations
        ));
        return SweepOutcome {
            should_iterate: false,
            // Blocked rather than failed: a human can raise the cap, and failing autonomously
            // would discard everything the run has produced.
            terminal: Some(RunPhase::BlockedOnHuman),
            notes,
        };
    }

    if state.spent_usd >= limits.max_cost_usd {
        notes.push(format!(
            "cost ceiling reached (${:.2} of ${:.2})",
            state.spent_usd, limits.max_cost_usd
        ));
        return SweepOutcome {
            should_iterate: false,
            terminal: Some(RunPhase::BlockedOnHuman),
            notes,
        };
    }

    if graph.objective_satisfied() {
        // Complete only once the branches have been merged and tested together. Every task
        // passing alone says nothing about whether they combine, and declaring success here
        // would hand the operator a green dashboard and an unresolved merge.
        if state.integrated {
            notes.push("every objective-gating task is complete and the branches integrate".into());
            return SweepOutcome {
                should_iterate: false,
                terminal: Some(RunPhase::Completed),
                notes,
            };
        }
        notes.push("every objective-gating task is complete; integrating the branches".into());
        return SweepOutcome {
            should_iterate: true,
            terminal: None,
            notes,
        };
    }

    // Deadlock: nothing ready, nothing in flight, nobody waiting on a human.
    if !graph.is_empty() && !graph.progress_possible(state.open_escalations) {
        notes.push(
            "no task is ready, running or under review and no escalation is open; the run \
             cannot progress on its own"
                .into(),
        );
        return SweepOutcome {
            should_iterate: false,
            terminal: Some(RunPhase::BlockedOnHuman),
            notes,
        };
    }

    // An empty graph needs the planner, whether or not anything else marked the run dirty.
    let needs_plan = graph.is_empty();
    let has_work = !graph.ready().is_empty()
        || graph
            .tasks()
            .any(|t| matches!(t.status, TaskStatus::Review | TaskStatus::Running));

    SweepOutcome {
        should_iterate: dirty || needs_plan || has_work,
        terminal: None,
        notes,
    }
}

/// Tracks which stages have committed, so a restart mid-iteration does not repeat side effects.
#[derive(Debug, Clone, Default)]
pub struct StageReceipts {
    completed: HashSet<(u32, Stage)>,
}

impl StageReceipts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, iteration: u32, stage: Stage) {
        self.completed.insert((iteration, stage));
    }

    pub fn is_done(&self, iteration: u32, stage: Stage) -> bool {
        self.completed.contains(&(iteration, stage))
    }

    /// Stages still to run for this iteration, in pipeline order.
    pub fn remaining(&self, iteration: u32) -> Vec<Stage> {
        Stage::PIPELINE
            .iter()
            .copied()
            .filter(|s| !self.is_done(iteration, *s))
            .collect()
    }
}

/// Records what a stage did. `Commit` closing an iteration is what advances the counter, so a
/// crash before it means the iteration is retried rather than half-counted.
#[derive(Debug, Clone, Default)]
pub struct IterationLog {
    pub decisions: Vec<DecisionRecord>,
}

impl IterationLog {
    pub fn record_code_decision(
        &mut self,
        iteration: u32,
        stage: Stage,
        kind: &str,
        rule_id: &str,
        rationale: &str,
    ) {
        self.decisions.push(DecisionRecord {
            iteration,
            stage: format!("{stage:?}"),
            kind: kind.to_string(),
            decided_by: DecidedBy::Code,
            rule_id: Some(rule_id.to_string()),
            task_id: None,
            inputs_digest: None,
            validation_errors: Vec::new(),
            repair_count: 0,
            rationale: rationale.to_string(),
            cost_usd: None,
        });
    }

    /// A decision the model made. `validation_errors` is non-empty when the first response was
    /// rejected, so the log shows the model needed correcting.
    #[allow(clippy::too_many_arguments)]
    pub fn record_model_decision(
        &mut self,
        iteration: u32,
        stage: Stage,
        kind: &str,
        rationale: &str,
        validation_errors: Vec<String>,
        repair_count: u32,
        cost_usd: Option<f64>,
    ) {
        self.decisions.push(DecisionRecord {
            iteration,
            stage: format!("{stage:?}"),
            kind: kind.to_string(),
            decided_by: DecidedBy::Claude,
            rule_id: None,
            task_id: None,
            inputs_digest: None,
            validation_errors,
            repair_count,
            rationale: rationale.to_string(),
            cost_usd,
        });
    }

    /// Total spend recorded this iteration, for the budget gate.
    pub fn cost(&self) -> f64 {
        self.decisions.iter().filter_map(|d| d.cost_usd).sum()
    }

    /// Decisions the model actually drove, as opposed to code fallbacks.
    pub fn model_decisions(&self) -> usize {
        self.decisions
            .iter()
            .filter(|d| d.decided_by == DecidedBy::Claude)
            .count()
    }
}
