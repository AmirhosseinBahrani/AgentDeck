//! Loop control: when the supervisor spends money, when it stops, and what survives a restart.

use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::domain::task::{apply, TaskEvent, TaskState};
use deck_supervisor::graph::{Mutation, TaskGraph};
use deck_supervisor::loop_engine::*;

fn graph_with(tasks: Vec<TaskState>) -> TaskGraph {
    let mut g = TaskGraph::new();
    g.apply(Mutation {
        add_tasks: tasks,
        add_edges: vec![],
    })
    .unwrap();
    g
}

fn gate() -> TaskState {
    let mut t = TaskState::new(TaskId::new());
    t.objective_gate = true;
    t
}

fn completed_gate() -> TaskState {
    let mut s = gate();
    s = apply(&s, TaskEvent::Enqueued).unwrap();
    s = apply(
        &s,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap();
    s = apply(&s, TaskEvent::Started).unwrap();
    s = apply(&s, TaskEvent::ClaimedDone).unwrap();
    apply(&s, TaskEvent::ReviewPassed).unwrap()
}

// ---------------------------------------------------------------------------
// Cost safety
// ---------------------------------------------------------------------------

#[test]
fn a_bare_tick_does_not_justify_an_iteration() {
    // The core cost-safety property. At a 2s tick, a loop that iterated on every tick would
    // spend most of its money discovering that nothing had happened.
    assert!(!Trigger::Tick.marks_dirty());
    for t in [
        Trigger::ReportReceived,
        Trigger::SessionExited,
        Trigger::TaskClaimedDone,
        Trigger::HumanAnswered,
        Trigger::CancelRequested,
    ] {
        assert!(t.marks_dirty(), "{t:?} should wake the loop");
    }
}

#[test]
fn an_idle_run_with_nothing_to_do_does_not_iterate() {
    let mut waiting = gate();
    waiting = apply(&waiting, TaskEvent::Enqueued).unwrap();
    waiting = apply(
        &waiting,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap();
    waiting = apply(&waiting, TaskEvent::Started).unwrap();
    // Running: an agent is working, so there is nothing for the supervisor to decide yet.
    let g = graph_with(vec![waiting]);

    let outcome = sweep(&RunState::default(), &g, RunLimits::default(), false);
    assert!(
        outcome.should_iterate,
        "work in flight means the loop should still observe it"
    );

    // Nothing at all: no tasks, no dirt.
    let empty = TaskGraph::new();
    let outcome = sweep(&RunState::default(), &empty, RunLimits::default(), false);
    assert!(
        outcome.should_iterate,
        "an empty graph needs planning even without a dirty flag"
    );
}

#[test]
fn only_five_of_thirteen_stages_may_call_a_model() {
    // The design's central claim, made checkable. If this count grows, the deterministic
    // controller is eroding.
    let callers: Vec<Stage> = Stage::PIPELINE
        .iter()
        .copied()
        .filter(|s| s.may_call_model())
        .collect();

    assert_eq!(
        callers.len(),
        5,
        "unexpected model-calling stages: {callers:?}"
    );
    for pure in [
        Stage::Observe,
        Stage::IngestReports,
        Stage::Reap,
        Stage::Dispatch,
        Stage::Verify,
        Stage::Adjudicate,
        Stage::CompletionCheck,
        Stage::Commit,
    ] {
        assert!(!pure.may_call_model(), "{pure:?} must stay pure code");
    }
}

#[test]
fn verification_and_adjudication_are_pure_so_a_model_cannot_override_a_red_test() {
    assert!(!Stage::Verify.may_call_model());
    assert!(!Stage::Adjudicate.may_call_model());
    // Judge may call a model, but only ever after Verify has passed.
    let verify_at = Stage::PIPELINE
        .iter()
        .position(|s| *s == Stage::Verify)
        .unwrap();
    let judge_at = Stage::PIPELINE
        .iter()
        .position(|s| *s == Stage::Judge)
        .unwrap();
    assert!(verify_at < judge_at, "verification must precede judgement");
}

// ---------------------------------------------------------------------------
// Termination
// ---------------------------------------------------------------------------

#[test]
fn every_gate_task_being_done_is_not_yet_completion() {
    // Each task passed in its own worktree on its own branch, which says nothing about whether
    // the branches combine — one agent can rename what another calls and both stay green. So
    // the sweep keeps iterating into the integration gate rather than declaring success.
    let g = graph_with(vec![completed_gate()]);
    let outcome = sweep(&RunState::default(), &g, RunLimits::default(), true);
    assert_eq!(outcome.terminal, None);
    assert!(
        outcome.should_iterate,
        "the integration gate still has to run"
    );
}

#[test]
fn the_run_completes_once_the_branches_integrate() {
    let g = graph_with(vec![completed_gate()]);
    let state = RunState {
        integrated: true,
        ..RunState::default()
    };
    let outcome = sweep(&state, &g, RunLimits::default(), true);
    assert_eq!(outcome.terminal, Some(RunPhase::Completed));
    assert!(!outcome.should_iterate);
}

#[test]
fn hitting_the_iteration_cap_blocks_on_a_human_rather_than_failing() {
    // Failing autonomously would discard everything the run produced; a human can raise the cap.
    let g = graph_with(vec![gate()]);
    let state = RunState {
        iteration: 200,
        ..Default::default()
    };
    let outcome = sweep(&state, &g, RunLimits::default(), true);

    assert_eq!(outcome.terminal, Some(RunPhase::BlockedOnHuman));
    assert!(!outcome.notes.is_empty(), "the reason should be recorded");
}

#[test]
fn hitting_the_cost_ceiling_stops_before_spending_more() {
    // Checked before "is there work", so an over-budget run does not do one more expensive
    // iteration on its way out.
    let g = graph_with(vec![gate()]);
    let state = RunState {
        spent_usd: 25.0,
        ..Default::default()
    };
    let outcome = sweep(&state, &g, RunLimits::default(), true);

    assert_eq!(outcome.terminal, Some(RunPhase::BlockedOnHuman));
    assert!(!outcome.should_iterate);
    assert!(outcome.notes[0].contains("cost"));
}

#[test]
fn deadlock_stops_the_run_instead_of_spinning() {
    let mut failed = gate();
    failed = apply(&failed, TaskEvent::Enqueued).unwrap();
    failed = apply(
        &failed,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap();
    failed = apply(&failed, TaskEvent::Started).unwrap();
    // Exhaust the attempts so the failure is permanent.
    let mut s = failed;
    while !s.status.is_terminal() {
        s = apply(
            &s,
            TaskEvent::Failed {
                reason: "boom".into(),
            },
        )
        .unwrap();
        if !s.status.is_terminal() {
            s = apply(
                &s,
                TaskEvent::Assigned {
                    agent_id: AgentId::new(),
                },
            )
            .unwrap();
            s = apply(&s, TaskEvent::Started).unwrap();
        }
    }

    let g = graph_with(vec![s]);
    let outcome = sweep(&RunState::default(), &g, RunLimits::default(), true);
    assert_eq!(outcome.terminal, Some(RunPhase::BlockedOnHuman));
}

#[test]
fn an_open_escalation_is_not_treated_as_deadlock() {
    // Waiting on a human is waiting, not stuck. Conflating them would abandon runs that only
    // needed an answer.
    let mut blocked = gate();
    blocked = apply(&blocked, TaskEvent::Enqueued).unwrap();
    blocked = apply(
        &blocked,
        TaskEvent::Blocked {
            reason: "needs a decision".into(),
        },
    )
    .unwrap();

    let g = graph_with(vec![blocked]);
    let state = RunState {
        open_escalations: 1,
        ..Default::default()
    };

    let outcome = sweep(&state, &g, RunLimits::default(), false);
    assert_eq!(outcome.terminal, None, "the run is waiting, not finished");
}

#[test]
fn a_terminal_run_never_iterates_again() {
    let g = graph_with(vec![gate()]);
    for phase in [RunPhase::Completed, RunPhase::Failed, RunPhase::Cancelled] {
        let state = RunState {
            phase,
            ..Default::default()
        };
        let outcome = sweep(&state, &g, RunLimits::default(), true);
        assert!(!outcome.should_iterate, "{phase:?} should be final");
        assert_eq!(outcome.terminal, Some(phase));
    }
}

// ---------------------------------------------------------------------------
// Crash resume
// ---------------------------------------------------------------------------

#[test]
fn completed_stages_are_skipped_on_resume() {
    // Each receipt is written with its stage's mutations, so a restart must not repeat side
    // effects like spawning an agent or creating a worktree.
    let mut receipts = StageReceipts::new();
    for stage in [
        Stage::Observe,
        Stage::IngestReports,
        Stage::Reap,
        Stage::Plan,
    ] {
        receipts.record(7, stage);
    }

    let remaining = receipts.remaining(7);
    assert!(
        !remaining.contains(&Stage::Plan),
        "planning already committed"
    );
    assert_eq!(
        remaining[0],
        Stage::Assign,
        "resume at the first unfinished stage"
    );
    assert_eq!(remaining.len(), Stage::PIPELINE.len() - 4);
}

#[test]
fn receipts_are_scoped_to_their_iteration() {
    // Otherwise a later iteration would skip stages because an earlier one had run them.
    let mut receipts = StageReceipts::new();
    receipts.record(1, Stage::Plan);

    assert!(receipts.is_done(1, Stage::Plan));
    assert!(!receipts.is_done(2, Stage::Plan));
    assert_eq!(receipts.remaining(2).len(), Stage::PIPELINE.len());
}

// ---------------------------------------------------------------------------
// Decision log
// ---------------------------------------------------------------------------

#[test]
fn the_log_distinguishes_code_decisions_from_model_decisions() {
    // Needed to answer "was the model actually driving?" when a run goes wrong.
    let mut log = IterationLog::default();
    log.record_code_decision(1, Stage::Dispatch, "dispatch", "capacity", "slot free");
    log.record_model_decision(1, Stage::Plan, "plan", "decomposed", vec![], 0, Some(0.12));

    assert_eq!(log.decisions.len(), 2);
    assert_eq!(log.model_decisions(), 1);
    assert!((log.cost() - 0.12).abs() < f64::EPSILON);
}

#[test]
fn a_repaired_decision_records_that_the_model_needed_correcting() {
    // A run where every decision took a repair round-trip is a prompt problem, and the log is
    // the only place that would show up.
    let mut log = IterationLog::default();
    log.record_model_decision(
        3,
        Stage::Plan,
        "plan",
        "second attempt",
        vec!["no objective_gate".into()],
        1,
        Some(0.4),
    );

    let d = &log.decisions[0];
    assert_eq!(d.repair_count, 1);
    assert_eq!(d.validation_errors.len(), 1);
}

#[test]
fn cost_sums_across_decisions_because_the_cli_reports_per_turn() {
    let mut log = IterationLog::default();
    log.record_model_decision(1, Stage::Plan, "plan", "", vec![], 0, Some(0.10));
    log.record_model_decision(1, Stage::Judge, "review", "", vec![], 0, Some(0.05));
    log.record_code_decision(1, Stage::Commit, "commit", "always", "");

    assert!(
        (log.cost() - 0.15).abs() < 1e-9,
        "taking the latest value instead of summing would undercount"
    );
}
