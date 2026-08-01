//! Autonomy is enforced where a process actually starts.
//!
//! The claim these tests defend is narrow and important: an autonomy mode is not a UI
//! preference. If it were only enforced in the frontend, "Manual" would mean the button looked
//! disabled while the supervisor spawned agents with edit rights into real worktrees anyway.
//! So every test here goes through the driver and asserts on whether an agent was *started*.

use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::domain::task::TaskStatus;
use deck_supervisor::autonomy::{ApprovalQueue, Autonomy};
use deck_supervisor::decision::PlanLimits;
use deck_supervisor::driver::{claim_done, Driver, IterationOutcome, Run, RunConfig, TeamMember};
use deck_supervisor::loop_engine::RunLimits;
use deck_supervisor::planner::ScriptedPlanner;
use deck_supervisor::workspaces::FakeWorkspaces;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-au-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn config(root: PathBuf, autonomy: Autonomy, test_cmd: &str) -> RunConfig {
    RunConfig {
        objective: "Ship the feature".into(),
        team: vec![TeamMember {
            agent_id: AgentId::new(),
            role: "developer".into(),
        }],
        default_test_command: test_cmd.into(),
        verification_root: root,
        limits: RunLimits::default(),
        plan_limits: PlanLimits::default(),
        per_call_budget_usd: 1.0,
        verification_timeout: Duration::from_secs(20),
        autonomy,
    }
}

/// One task whose verification outcome the test chooses.
fn one_task_plan(cmd: &str) -> serde_json::Value {
    json!({
        "tasks": [{
            "tmp_id": "a", "title": "Task A", "role": "developer",
            "objective_gate": true, "description": "",
            "contract": {
                "version": 1,
                "acceptance_criteria": [{
                    "id": "a-check", "text": "it works",
                    "verify": { "type": "command", "cmd": cmd, "expect_exit_zero": true }
                }],
                "constraints": [], "deliverables": [], "definition_of_done": "done"
            }
        }],
        "edges": [], "reasoning": "one task"
    })
}

/// Approvals a test hands over, in order.
#[derive(Default)]
struct Granted(parking_lot::Mutex<Vec<TaskId>>);

impl Granted {
    fn grant(&self, id: TaskId) {
        self.0.lock().push(id);
    }
}

impl ApprovalQueue for Granted {
    fn drain(&self) -> Vec<TaskId> {
        std::mem::take(&mut *self.0.lock())
    }
}

#[tokio::test]
async fn an_assisted_run_plans_and_assigns_but_starts_nothing() {
    // Planning is not the dangerous part. Assisted mode still does the thinking, so the operator
    // has something concrete to approve rather than an empty screen and a prompt.
    let root = workdir("assisted-holds");
    let cfg = config(root.clone(), Autonomy::Assisted, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };

    assert!(dispatched.is_empty(), "nothing may start without approval");
    assert_eq!(
        run.awaiting_approval.len(),
        1,
        "and it must say what is held"
    );
    assert_eq!(
        run.graph.tasks().count(),
        1,
        "the task was still planned and assigned"
    );
    assert_eq!(
        run.graph.tasks().next().unwrap().status,
        TaskStatus::Assigned,
        "assigned, not running"
    );
}

#[tokio::test]
async fn approving_a_task_starts_exactly_that_agent() {
    let root = workdir("assisted-approve");
    let cfg = config(root.clone(), Autonomy::Assisted, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let approvals = Granted::default();

    let driver = Driver::new(&cfg, &planner, &workspaces).with_approvals(&approvals);
    let mut run = Run::new();

    driver.step(&mut run, true).await;
    let held = *run.awaiting_approval.first().expect("one task held");

    approvals.grant(held);
    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };

    assert_eq!(dispatched, vec![held]);
    assert!(run.awaiting_approval.is_empty());
    assert_eq!(
        run.graph.get(held).unwrap().status,
        TaskStatus::Running,
        "an approved task should actually be running"
    );
}

#[tokio::test]
async fn an_approval_authorises_one_start_and_not_the_next() {
    // The subtle failure: if approval were a sticky flag rather than a token, approving once
    // would silently authorise every future retry of the same task — which is precisely the
    // unattended behaviour the operator chose Assisted to avoid.
    let root = workdir("one-shot");
    // A criterion that cannot pass, so the task comes back for another attempt.
    let cfg = config(root.clone(), Autonomy::Assisted, "false");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("false"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let approvals = Granted::default();

    let driver = Driver::new(&cfg, &planner, &workspaces).with_approvals(&approvals);
    let mut run = Run::new();

    driver.step(&mut run, true).await;
    let held = *run.awaiting_approval.first().expect("one task held");
    approvals.grant(held);
    driver.step(&mut run, true).await;
    assert_eq!(run.graph.get(held).unwrap().status, TaskStatus::Running);

    // The agent claims done, the gate fails it, and it becomes dispatchable again.
    claim_done(&mut run, held);
    driver.step(&mut run, true).await;
    driver.step(&mut run, true).await;

    assert_ne!(
        run.graph.get(held).unwrap().status,
        TaskStatus::Running,
        "the retry must not start on the spent approval"
    );
}

#[tokio::test]
async fn an_autonomous_run_never_waits_for_approval() {
    let root = workdir("autonomous");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    assert_eq!(dispatched.len(), 1);
    assert!(run.awaiting_approval.is_empty());
}

#[tokio::test]
async fn a_manual_run_hands_a_failure_back_instead_of_retrying_it() {
    // Manual's claim is that nothing happens twice without a human seeing it happen once. A
    // silent retry after a red test is exactly that, so the failure has to block and escalate.
    let root = workdir("manual-fail");
    let cfg = config(root.clone(), Autonomy::Manual, "false");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("false"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let approvals = Granted::default();

    let driver = Driver::new(&cfg, &planner, &workspaces).with_approvals(&approvals);
    let mut run = Run::new();

    driver.step(&mut run, true).await;
    let held = *run.awaiting_approval.first().expect("one task held");
    approvals.grant(held);
    driver.step(&mut run, true).await;

    claim_done(&mut run, held);
    driver.step(&mut run, true).await;

    assert_eq!(
        run.graph.get(held).unwrap().status,
        TaskStatus::Blocked,
        "a failure in manual mode blocks rather than queueing another attempt"
    );
    assert!(
        run.state.open_escalations > 0,
        "and it asks for a human rather than stalling quietly"
    );
}

#[tokio::test]
async fn an_assisted_run_still_retries_a_failure_on_its_own() {
    // The counterpart to the test above: only Manual suppresses retries. Assisted is about who
    // starts an agent, not about whether the supervisor may try again.
    let root = workdir("assisted-retry");
    let cfg = config(root.clone(), Autonomy::Assisted, "false");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("false"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let approvals = Granted::default();

    let driver = Driver::new(&cfg, &planner, &workspaces).with_approvals(&approvals);
    let mut run = Run::new();

    driver.step(&mut run, true).await;
    let held = *run.awaiting_approval.first().expect("one task held");
    approvals.grant(held);
    driver.step(&mut run, true).await;

    claim_done(&mut run, held);
    driver.step(&mut run, true).await;

    assert_ne!(
        run.graph.get(held).unwrap().status,
        TaskStatus::Blocked,
        "assisted mode should still be willing to try again"
    );
}

// ---------------------------------------------------------------------------
// The integration gate — what the run does with each outcome
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_run_is_not_complete_until_the_branches_integrate() {
    // Every task green in its own worktree is not the same as the work combining. Declaring
    // completion here would hand the operator a green dashboard and an unmerged pile of
    // branches, which is the failure the gate exists to prevent.
    use deck_supervisor::loop_engine::RunPhase;

    let root = workdir("integrates");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    claim_done(&mut run, dispatched[0]);

    // Verification and integration happen in the same pass, because CompletionCheck sits after
    // Verify in the pipeline. Nothing waits an extra iteration to learn that the branches merge.
    driver.step(&mut run, true).await;
    assert_eq!(
        run.graph.get(dispatched[0]).unwrap().status,
        TaskStatus::Completed
    );
    assert!(run.state.integrated, "the gate runs in the same iteration");

    // Only now may the sweep call the run complete.
    assert!(matches!(
        driver.step(&mut run, true).await,
        IterationOutcome::Terminal(RunPhase::Completed)
    ));
}

#[tokio::test]
async fn a_merge_conflict_stops_the_run_and_asks_a_human() {
    // A conflict means two agents were given overlapping work. Resolving it would mean choosing
    // whose work to discard, which is not a decision code or a model should make silently.
    use deck_core::git::IntegrationOutcome;
    use deck_supervisor::loop_engine::RunPhase;

    let root = workdir("conflict");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    workspaces.set_integration(IntegrationOutcome::Conflicted {
        branch: "agentdeck/task-abc".into(),
        task_id: "abc".into(),
        files: vec!["src/lib.rs".into()],
    });

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    claim_done(&mut run, dispatched[0]);
    driver.step(&mut run, true).await;
    driver.step(&mut run, true).await;

    assert!(!run.state.integrated);
    assert_eq!(run.state.phase, RunPhase::BlockedOnHuman);
    assert!(run.state.open_escalations > 0);
    assert!(
        run.log
            .decisions
            .iter()
            .any(|d| d.kind == "integration_conflict" && d.rationale.contains("src/lib.rs")),
        "the operator needs to be told which file clashed"
    );
}

#[tokio::test]
async fn branches_that_merge_but_break_together_block_rather_than_blaming_a_task() {
    // No individual task is at fault — each one passed. Reopening one would send an agent to
    // fix code that is correct on its own.
    use deck_core::git::IntegrationOutcome;
    use deck_supervisor::loop_engine::RunPhase;

    let root = workdir("broken-combo");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    workspaces.set_integration(IntegrationOutcome::TestsFailed {
        output: "3 tests failed after merging".into(),
    });

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task = dispatched[0];
    claim_done(&mut run, task);
    driver.step(&mut run, true).await;
    driver.step(&mut run, true).await;

    assert_eq!(run.state.phase, RunPhase::BlockedOnHuman);
    assert_eq!(
        run.graph.get(task).unwrap().status,
        TaskStatus::Completed,
        "the task itself passed and must not be reopened"
    );
}

// ---------------------------------------------------------------------------
// Reap — agents that die without saying anything
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_agent_that_dies_without_claiming_done_fails_its_task() {
    // `claim_task_done` is the only legal completion path, so a session that simply exits has
    // failed rather than quietly succeeded. Without the reap the task sits in Running forever:
    // nothing will report on it, no trigger arrives, and the run stalls with no sign of why.
    let root = workdir("reap");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task = dispatched[0];
    assert_eq!(run.graph.get(task).unwrap().status, TaskStatus::Running);

    workspaces.kill_agent(task);
    driver.step(&mut run, true).await;

    assert!(
        run.log.decisions.iter().any(|d| d.kind == "agent_died"),
        "the death has to be noticed and recorded"
    );
    assert_eq!(
        run.graph.get(task).unwrap().attempts,
        2,
        "the lost attempt is spent, so this cannot loop forever"
    );
}

#[tokio::test]
async fn a_task_that_keeps_losing_its_agent_eventually_fails_and_asks_for_a_human() {
    // The counterpart to retrying: without an end, a task whose agent dies on every attempt
    // would be re-dispatched forever and quietly drain the account's usage allowance.
    let root = workdir("reap-exhausted");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task = dispatched[0];

    // The agent dies on every attempt.
    for _ in 0..4 {
        workspaces.kill_agent(task);
        driver.step(&mut run, true).await;
    }

    assert_eq!(
        run.graph.get(task).unwrap().status,
        TaskStatus::Failed,
        "it has to stop trying"
    );
    assert!(
        run.state.open_escalations > 0,
        "and say so, rather than failing quietly"
    );
}

#[tokio::test]
async fn a_reaped_task_with_attempts_left_is_dispatched_again() {
    let root = workdir("reap-retry");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task = dispatched[0];
    workspaces.kill_agent(task);

    // Reaped and re-queued in one pass; the next dispatch picks it back up.
    driver.step(&mut run, true).await;
    assert_eq!(
        workspaces.dispatch_count(),
        2,
        "the task should have been given another agent"
    );
    assert_eq!(run.graph.get(task).unwrap().attempts, 2);
}

#[tokio::test]
async fn a_task_that_is_merely_waiting_is_never_reaped() {
    // `None` means no agent was ever started, which is a scheduling state rather than a death.
    // Treating it as one would fail every task the moment it was assigned.
    let root = workdir("reap-waiting");
    let cfg = config(root.clone(), Autonomy::Assisted, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    driver.step(&mut run, true).await;
    driver.step(&mut run, true).await;

    let task = run.graph.tasks().next().unwrap();
    assert_eq!(
        task.status,
        TaskStatus::Assigned,
        "an unstarted task must survive the reap"
    );
    assert_eq!(task.attempts, 0);
}

// ---------------------------------------------------------------------------
// Escalations — the run has to be answerable, and the answer has to land
// ---------------------------------------------------------------------------

/// Answers a test hands to the driver.
#[derive(Default)]
struct Answers(parking_lot::Mutex<Vec<(String, deck_supervisor::escalation::EscalationAnswer)>>);

impl deck_supervisor::escalation::AnswerQueue for Answers {
    fn drain(&self) -> Vec<(String, deck_supervisor::escalation::EscalationAnswer)> {
        std::mem::take(&mut *self.0.lock())
    }
}

#[tokio::test]
async fn a_blocked_task_raises_a_question_with_answers_attached() {
    // The failure this replaces: the run parked, the dashboard said a decision was needed, and
    // there was no question and nothing to click.
    use deck_core::git::IntegrationOutcome;
    use deck_supervisor::loop_engine::RunPhase;

    let root = workdir("escalates");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    workspaces.set_integration(IntegrationOutcome::TestsFailed {
        output: "3 tests failed after merging".into(),
    });

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    claim_done(&mut run, dispatched[0]);
    driver.step(&mut run, true).await;

    assert_eq!(run.state.phase, RunPhase::BlockedOnHuman);
    let escalation = run.escalations.first().expect("a question must be raised");
    assert!(!escalation.question.trim().is_empty());
    assert!(
        !escalation.options.is_empty(),
        "an unanswerable question is the bug this replaces"
    );
    assert!(escalation.detail.contains("3 tests failed"));
}

#[tokio::test]
async fn answering_unparks_the_run() {
    // "It will resume once you answer" has to be true.
    use deck_core::git::IntegrationOutcome;
    use deck_supervisor::escalation::EscalationAnswer;
    use deck_supervisor::loop_engine::RunPhase;

    let root = workdir("unparks");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    workspaces.set_integration(IntegrationOutcome::TestsFailed {
        output: "broken together".into(),
    });
    let answers = Answers::default();

    let driver = Driver::new(&cfg, &planner, &workspaces).with_answers(&answers);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    claim_done(&mut run, dispatched[0]);
    driver.step(&mut run, true).await;
    let id = run.escalations[0].id.clone();

    answers.0.lock().push((id, EscalationAnswer::Reintegrate));
    driver.step(&mut run, true).await;

    assert!(run.escalations.is_empty(), "the question should be closed");
    assert_ne!(
        run.state.phase,
        RunPhase::BlockedOnHuman,
        "the run must actually resume"
    );
}

#[tokio::test]
async fn retrying_a_task_refunds_the_attempt_that_failed() {
    // Otherwise "try again" is refused immediately by the very cap that raised the question.
    use deck_supervisor::escalation::{EscalationAnswer, EscalationKind};

    let root = workdir("refund");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task = dispatched[0];
    let spent = run.graph.get(task).unwrap().attempts;

    run.escalate(EscalationKind::AttemptsExhausted, Some(task), "q", "d");
    let id = run.escalations[0].id.clone();
    assert!(run.answer(&id, EscalationAnswer::RetryTask { task_id: task }));

    assert_eq!(run.graph.get(task).unwrap().attempts, spent - 1);
}

#[tokio::test]
async fn the_same_question_is_not_asked_twice() {
    // The sweep runs every couple of seconds. Without dedup an unanswered question would pile
    // up until the inbox was unreadable.
    use deck_supervisor::escalation::EscalationKind;

    let root = workdir("dedup");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    let workspaces = FakeWorkspaces::new(root);
    let _driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    for _ in 0..5 {
        run.escalate(EscalationKind::IntegrationBroken, None, "same", "detail");
    }
    assert_eq!(run.escalations.len(), 1);
}

#[tokio::test]
async fn answering_an_unknown_id_changes_nothing() {
    // A double click or a stale window must not apply an answer twice.
    use deck_supervisor::escalation::{EscalationAnswer, EscalationKind};

    let mut run = Run::new();
    run.escalate(EscalationKind::IntegrationBroken, None, "q", "d");
    assert!(!run.answer("not-a-real-id", EscalationAnswer::Reintegrate));
    assert_eq!(run.escalations.len(), 1, "the real question must survive");
}
