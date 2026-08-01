//! Being blocked on a human has to be answerable, and answering has to resume the run.
//!
//! The behaviour these pin down replaces a dead end. The dashboard said "waiting for a decision
//! from you — it will resume once you answer", and that sentence was false twice: an escalation
//! was a bare counter with no question attached, and `BlockedOnHuman` was returned as a terminal
//! phase, so the loop had already exited. Nothing could be answered and nothing was left to
//! resume.

use deck_core::domain::ids::TaskId;
use deck_supervisor::escalation::{Escalation, EscalationAnswer, EscalationKind};
use deck_supervisor::graph::TaskGraph;
use deck_supervisor::loop_engine::{sweep, RunLimits, RunPhase, RunState};

#[test]
fn being_blocked_on_a_human_does_not_end_the_run() {
    // The bug in one assertion. A terminal phase makes RunLoop return, so no later answer could
    // ever reach it.
    let state = RunState {
        spent_usd: 100.0,
        ..RunState::default()
    };
    let outcome = sweep(&state, &TaskGraph::new(), RunLimits::default(), true);

    assert_eq!(
        outcome.terminal, None,
        "the run must stay alive to be answerable"
    );
    assert_eq!(outcome.phase, Some(RunPhase::BlockedOnHuman));
    assert!(
        !outcome.should_iterate,
        "but it must not keep spending either"
    );
}

#[test]
fn a_finished_run_still_ends() {
    // The counterpart: parking must not have made termination impossible.
    let state = RunState {
        phase: RunPhase::Completed,
        ..RunState::default()
    };
    let outcome = sweep(&state, &TaskGraph::new(), RunLimits::default(), true);
    assert_eq!(outcome.terminal, Some(RunPhase::Completed));
}

#[test]
fn every_escalation_offers_at_least_one_answer() {
    // A question with no answers is the dead end this work exists to remove, so it must be
    // impossible to construct one — for every kind, not just the ones wired up today.
    let kinds = [
        EscalationKind::PlanningFailed,
        EscalationKind::DispatchFailed,
        EscalationKind::TaskBlocked,
        EscalationKind::AttemptsExhausted,
        EscalationKind::IntegrationConflict,
        EscalationKind::IntegrationBroken,
        EscalationKind::LimitReached,
    ];
    for kind in kinds {
        let e = Escalation::new(kind, Some(TaskId::new()), "q", "d", 0);
        assert!(!e.options.is_empty(), "{kind:?} offered nothing to click");
    }
}

#[test]
fn a_task_scoped_answer_is_never_offered_without_a_task() {
    // Retrying "the task" when there is no task would be a button that silently does nothing.
    let e = Escalation::new(EscalationKind::TaskBlocked, None, "q", "d", 0);
    assert!(
        e.options
            .iter()
            .all(|o| !matches!(o.answer, EscalationAnswer::RetryTask { .. })),
        "a retry was offered with nothing to retry"
    );
}

#[test]
fn ending_the_run_is_always_available_and_always_marked_destructive() {
    // Whatever else is on offer, the operator must be able to stop — and must be able to see
    // that stopping is the answer that throws work away.
    for kind in [EscalationKind::PlanningFailed, EscalationKind::TaskBlocked] {
        let e = Escalation::new(kind, Some(TaskId::new()), "q", "d", 0);
        let cancel = e
            .options
            .iter()
            .find(|o| o.answer == EscalationAnswer::CancelRun)
            .expect("ending the run must always be possible");
        assert!(cancel.destructive);
    }
}

#[test]
fn every_option_explains_what_it_will_do() {
    // These answers abandon tasks and end runs. A bare verb is not enough to choose between them.
    let e = Escalation::new(
        EscalationKind::AttemptsExhausted,
        Some(TaskId::new()),
        "q",
        "d",
        0,
    );
    for option in &e.options {
        assert!(!option.label.trim().is_empty());
        assert!(
            option.consequence.trim().len() > 20,
            "{:?} does not say what it does",
            option.label
        );
    }
}
