//! Task transition rules.
//!
//! These encode the spec's constraint that the application, not a model, owns state. The
//! interesting tests are the ones that assert something is *refused*, and the ones about
//! counters — because the counters are what terminate the retry and rework loops.

use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::domain::task::{
    apply, is_dispatchable, TaskEvent, TaskLimits, TaskState, TaskStatus, TransitionError,
};

fn fresh() -> TaskState {
    TaskState::new(TaskId::new())
}

fn assigned() -> TaskState {
    let s = fresh();
    let s = apply(&s, TaskEvent::Enqueued).unwrap();
    apply(
        &s,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap()
}

fn running() -> TaskState {
    apply(&assigned(), TaskEvent::Started).unwrap()
}

#[test]
fn the_happy_path_reaches_completed() {
    let s = running();
    let s = apply(&s, TaskEvent::ClaimedDone).unwrap();
    assert_eq!(s.status, TaskStatus::Review);
    let s = apply(&s, TaskEvent::ReviewPassed).unwrap();
    assert_eq!(s.status, TaskStatus::Completed);
}

#[test]
fn only_starting_increments_attempts() {
    // Attempts must count dispatches, not failures, or the retry cap would be unbounded when a
    // task fails in a way that never reaches Started.
    let s = assigned();
    assert_eq!(s.attempts, 0);

    let s = apply(&s, TaskEvent::Started).unwrap();
    assert_eq!(s.attempts, 1);

    let s = apply(
        &s,
        TaskEvent::Failed {
            reason: "boom".into(),
        },
    )
    .unwrap();
    assert_eq!(s.attempts, 1, "failing must not double-count the attempt");
}

#[test]
fn attempts_are_exhausted_and_then_the_task_fails_permanently() {
    let mut s = assigned();
    let limit = s.limits.max_attempts;

    for expected in 1..=limit {
        s = apply(&s, TaskEvent::Started).unwrap();
        assert_eq!(s.attempts, expected);
        s = apply(
            &s,
            TaskEvent::Failed {
                reason: "flaky".into(),
            },
        )
        .unwrap();

        if expected < limit {
            assert_eq!(s.status, TaskStatus::Queued, "should be retried");
            s = apply(
                &s,
                TaskEvent::Assigned {
                    agent_id: AgentId::new(),
                },
            )
            .unwrap();
        }
    }

    assert_eq!(
        s.status,
        TaskStatus::Failed,
        "the retry loop must terminate rather than cycling forever"
    );
    assert_eq!(s.attempts_remaining(), 0);
}

#[test]
fn a_rejected_review_returns_for_rework_until_the_round_cap() {
    let mut s = apply(&running(), TaskEvent::ClaimedDone).unwrap();
    let cap = s.limits.max_review_rounds;

    for round in 1..=cap {
        s = apply(
            &s,
            TaskEvent::ReviewFailed {
                reason: "tests fail".into(),
            },
        )
        .unwrap();
        assert_eq!(s.review_rounds, round);

        if round < cap {
            assert_eq!(
                s.status,
                TaskStatus::Assigned,
                "should go back for a fix, not fail immediately"
            );
            s = apply(&s, TaskEvent::Started).unwrap();
            s = apply(&s, TaskEvent::ClaimedDone).unwrap();
        }
    }

    assert_eq!(
        s.status,
        TaskStatus::Failed,
        "review/rework must not ping-pong forever"
    );
    assert!(
        s.failure_reason.is_some(),
        "a permanent failure needs a reason"
    );
}

#[test]
fn cancelling_does_not_consume_a_retry_attempt() {
    // Cancellation is user intent, not agent fault. Counting it would make an operator's stop
    // look like a repeat failure and could push a healthy task over its retry cap.
    let s = running();
    let attempts_before = s.attempts;

    let s = apply(&s, TaskEvent::Cancelled).unwrap();
    assert_eq!(s.status, TaskStatus::Cancelled);
    assert_eq!(
        s.attempts, attempts_before,
        "a user-initiated stop must not cost a retry"
    );
}

#[test]
fn cancellation_is_legal_from_every_non_terminal_state() {
    // The operator must always be able to stop an agent, whatever it is doing.
    let states = [
        fresh(),
        apply(&fresh(), TaskEvent::Enqueued).unwrap(),
        assigned(),
        running(),
        apply(&running(), TaskEvent::ClaimedDone).unwrap(),
        apply(
            &running(),
            TaskEvent::Blocked {
                reason: "waiting".into(),
            },
        )
        .unwrap(),
    ];

    for state in states {
        let from = state.status;
        let result = apply(&state, TaskEvent::Cancelled);
        assert!(
            result.is_ok(),
            "cancel should be legal from {from:?}, got {result:?}"
        );
    }
}

#[test]
fn terminal_states_cannot_be_reopened() {
    // Reopening would let a replan resurrect work whose worktree may already be gone.
    for terminal in [
        apply(
            &apply(&running(), TaskEvent::ClaimedDone).unwrap(),
            TaskEvent::ReviewPassed,
        )
        .unwrap(),
        apply(&running(), TaskEvent::Cancelled).unwrap(),
    ] {
        let status = terminal.status;
        for event in [
            TaskEvent::Enqueued,
            TaskEvent::Started,
            TaskEvent::ClaimedDone,
            TaskEvent::ReviewPassed,
            TaskEvent::Cancelled,
            TaskEvent::Unblocked,
        ] {
            let result = apply(&terminal, event.clone());
            assert!(
                matches!(result, Err(TransitionError::Terminal(_))),
                "{event:?} should be refused from terminal {status:?}, got {result:?}"
            );
        }
    }
}

#[test]
fn a_task_cannot_start_without_an_assignee() {
    // Otherwise a dispatch bug would spawn an agent with no owner and no worktree.
    let mut s = apply(&fresh(), TaskEvent::Enqueued).unwrap();
    s.status = TaskStatus::Assigned; // force the status without setting an assignee
    s.assignee = None;

    assert!(matches!(
        apply(&s, TaskEvent::Started),
        Err(TransitionError::Unassigned)
    ));
}

#[test]
fn work_cannot_be_claimed_done_before_it_starts() {
    // `claim_task_done` is the only legal completion path, so it must not be reachable from a
    // state where no session ever ran.
    for state in [
        fresh(),
        apply(&fresh(), TaskEvent::Enqueued).unwrap(),
        assigned(),
    ] {
        let from = state.status;
        assert!(
            matches!(
                apply(&state, TaskEvent::ClaimedDone),
                Err(TransitionError::Illegal { .. })
            ),
            "ClaimedDone should be illegal from {from:?}"
        );
    }
}

#[test]
fn review_cannot_pass_without_a_review() {
    // A model could otherwise assert success straight from Running and skip verification.
    assert!(matches!(
        apply(&running(), TaskEvent::ReviewPassed),
        Err(TransitionError::Illegal { .. })
    ));
}

#[test]
fn a_failed_dependency_blocks_rather_than_fails_the_task() {
    // A fix task may still unblock it; failing here would discard work that is only waiting.
    let blocking = TaskId::new();
    let s = apply(&assigned(), TaskEvent::DependencyFailed { blocking }).unwrap();

    assert_eq!(s.status, TaskStatus::Blocked);
    assert_eq!(s.blocking_task, Some(blocking));
    assert!(s.failure_reason.is_some());
}

#[test]
fn unblocking_returns_to_the_queue_not_straight_to_running() {
    // Capacity and resource locks have to be re-acquired, and the previous assignee may no
    // longer be free.
    let s = apply(
        &running(),
        TaskEvent::Blocked {
            reason: "waiting on API".into(),
        },
    )
    .unwrap();
    let s = apply(&s, TaskEvent::Unblocked).unwrap();

    assert_eq!(s.status, TaskStatus::Queued);
    assert!(s.failure_reason.is_none(), "the blocker should be cleared");
    assert!(
        !is_dispatchable(&s),
        "requeued work must be re-assigned first"
    );
}

#[test]
fn a_blocked_task_can_be_reassigned_to_a_different_agent() {
    let s = apply(
        &running(),
        TaskEvent::Blocked {
            reason: "stuck".into(),
        },
    )
    .unwrap();
    let other = AgentId::new();
    let s = apply(&s, TaskEvent::Assigned { agent_id: other }).unwrap();

    assert_eq!(s.status, TaskStatus::Assigned);
    assert_eq!(s.assignee, Some(other));
    assert!(s.blocking_task.is_none());
}

#[test]
fn only_assigned_tasks_are_dispatchable() {
    assert!(!is_dispatchable(&fresh()));
    assert!(!is_dispatchable(
        &apply(&fresh(), TaskEvent::Enqueued).unwrap()
    ));
    assert!(is_dispatchable(&assigned()));
    assert!(!is_dispatchable(&running()), "already running");
}

#[test]
fn limits_are_configurable_and_respected() {
    // A single-attempt task should fail on its first failure rather than silently retrying.
    let mut s = fresh();
    s.limits = TaskLimits {
        max_attempts: 1,
        max_review_rounds: 1,
    };
    let s = apply(&s, TaskEvent::Enqueued).unwrap();
    let s = apply(
        &s,
        TaskEvent::Assigned {
            agent_id: AgentId::new(),
        },
    )
    .unwrap();
    let s = apply(&s, TaskEvent::Started).unwrap();
    let s = apply(
        &s,
        TaskEvent::Failed {
            reason: "once".into(),
        },
    )
    .unwrap();

    assert_eq!(s.status, TaskStatus::Failed);
}

#[test]
fn transitions_are_pure_so_a_refusal_leaves_the_original_untouched() {
    // The supervisor replays decisions against this table as a regression corpus, which only
    // works if apply() never mutates its input.
    let before = running();
    let snapshot = before.clone();

    let _ = apply(&before, TaskEvent::ReviewPassed);
    let _ = apply(&before, TaskEvent::Enqueued);

    assert_eq!(before, snapshot, "apply() must not mutate its input");
}

#[test]
fn active_and_terminal_classifications_match_the_lifecycle() {
    assert!(TaskStatus::Running.is_active());
    assert!(
        TaskStatus::Review.is_active(),
        "review still holds a worktree"
    );
    assert!(!TaskStatus::Queued.is_active());

    for s in [
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
    ] {
        assert!(s.is_terminal());
        assert!(!s.is_active());
    }
}
