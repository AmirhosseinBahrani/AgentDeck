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
        default_test_command: Some(test_cmd.into()),
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
async fn a_manual_run_plans_and_assigns_but_starts_nothing() {
    // Planning is not the dangerous part. Manual mode still does the thinking, so the operator
    // has something concrete to approve rather than an empty screen and a prompt.
    let root = workdir("manual-holds");
    let cfg = config(root.clone(), Autonomy::Manual, "true");
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
    let root = workdir("manual-approve");
    let cfg = config(root.clone(), Autonomy::Manual, "true");
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
    // unattended behaviour the operator chose Manual to avoid.
    let root = workdir("one-shot");
    // A criterion that cannot pass, so the task comes back for another attempt.
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
    // Retrying is what still separates Assisted from Manual now that neither the operator nor
    // the supervisor is asked before an agent starts.
    let root = workdir("assisted-retry");
    let cfg = config(root.clone(), Autonomy::Assisted, "false");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("false"), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    assert_eq!(
        dispatched.len(),
        1,
        "assisted starts its own agents — only Manual holds them"
    );
    assert!(run.awaiting_approval.is_empty(), "and holds nothing back");

    let task = dispatched[0];
    claim_done(&mut run, task);
    driver.step(&mut run, true).await;

    assert_ne!(
        run.graph.get(task).unwrap().status,
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
    let cfg = config(root.clone(), Autonomy::Manual, "true");
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

#[tokio::test]
async fn no_more_than_the_cap_may_be_working_at_once() {
    // There was no cap at all: every assigned task dispatched, so a wide plan spawned every agent
    // in the same instant. On subscription billing that shows up as a wave of throttled agents
    // rather than as one legible "too many at once", which is a far harder thing to diagnose.
    let root = workdir("concurrency-cap");
    let mut cfg = config(root.clone(), Autonomy::Autonomous, "true");
    cfg.limits.max_concurrent_agents = 2;

    let planner = ScriptedPlanner::new();
    planner.push(
        json!({
            "tasks": (0..5).map(|i| json!({
                "tmp_id": format!("t{i}"), "title": format!("Task {i}"), "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [{
                        "id": format!("c{i}"), "text": "it works",
                        "verify": { "type": "command", "cmd": "true", "expect_exit_zero": true }
                    }],
                    "constraints": [], "deliverables": [], "definition_of_done": "done"
                }
            })).collect::<Vec<_>>(),
            "edges": [], "reasoning": "five independent tasks"
        }),
        0.10,
    );

    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };

    assert_eq!(dispatched.len(), 2, "the cap is the cap");
    assert_eq!(
        run.graph
            .tasks()
            .filter(|t| t.status == TaskStatus::Assigned)
            .count(),
        3,
        "the rest stay assigned and wait for a slot rather than failing or escalating"
    );
}

#[tokio::test]
async fn tasks_spread_across_agents_that_share_a_role() {
    // Hiring three frontend engineers only means anything if the graph is spread across them.
    // The model used to be asked which one should take each task, given the task's *id* and no
    // roster — nothing to reason from, so it named the same agent every time and two of the three
    // sat idle. Code decides while anyone is free.
    let root = workdir("spread");
    let mut cfg = config(root.clone(), Autonomy::Autonomous, "true");
    cfg.team = (0..3)
        .map(|_| TeamMember {
            agent_id: AgentId::new(),
            role: "developer".into(),
        })
        .collect();

    let planner = ScriptedPlanner::new();
    planner.push(
        json!({
            "tasks": (0..3).map(|i| json!({
                "tmp_id": format!("t{i}"), "title": format!("Task {i}"), "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [{
                        "id": format!("c{i}"), "text": "it works",
                        "verify": { "type": "command", "cmd": "true", "expect_exit_zero": true }
                    }],
                    "constraints": [], "deliverables": [], "definition_of_done": "done"
                }
            })).collect::<Vec<_>>(),
            "edges": [], "reasoning": "three independent tasks"
        }),
        0.10,
    );

    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    let mut per_agent = std::collections::HashMap::new();
    for task in run.graph.tasks() {
        if let Some(agent) = task.assignee {
            *per_agent.entry(agent).or_insert(0) += 1;
        }
    }

    assert_eq!(per_agent.len(), 3, "one task each, not three on one agent");
    assert!(per_agent.values().all(|&n| n == 1));
}

#[tokio::test]
async fn polling_a_working_agent_does_not_consume_iterations() {
    // The sweep asks for an iteration whenever any task is Running, which is the normal condition
    // for an agent doing its job. Counting each of those passes meant a two-second tick spent the
    // whole 200-iteration budget in about seven minutes and stopped the run with "iteration cap
    // reached" — a message about looping, produced by a run that had looped over nothing.
    let root = workdir("iteration-poll");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    // Plan and dispatch: real progress, and worth counting.
    driver.step(&mut run, true).await;
    let after_dispatch = run.state.productive;
    assert!(after_dispatch > 0, "planning and dispatching is progress");

    // The agent is now working and reports nothing. Every pass from here changes nothing.
    for _ in 0..10 {
        driver.step(&mut run, false).await;
    }

    assert_eq!(
        run.state.productive, after_dispatch,
        "ten polls of an unchanged run must not spend ten iterations"
    );
    assert!(
        run.state.iteration > run.state.productive,
        "the pass counter still advances — stage receipts are keyed by it, and reusing a number \
         would leave every stage already recorded and run nothing"
    );
}

#[tokio::test]
async fn an_agent_that_finishes_without_claiming_is_nudged_then_failed() {
    // The reported symptom: "developer is done but says running". `claim_task_done` is the only
    // legal completion path, and the usual way to miss it is not failure but forgetfulness — the
    // agent finishes, writes "Done — the script prints Hello, World!", and stops. Its process is
    // still up, so reap sees no death, and the task sits in Running for the rest of the run
    // looking exactly like an agent still working.
    let root = workdir("silent-agent");
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

    // Quiet for a while, but not long enough to give up on.
    workspaces.set_idle(task, Duration::from_secs(200));
    driver.step(&mut run, true).await;

    assert_eq!(workspaces.nudges(), vec![task], "reminded, not discarded");
    assert_eq!(
        run.graph.get(task).unwrap().status,
        TaskStatus::Running,
        "a nudge must not itself end the task"
    );

    // Nudged again on the next pass would be the loop this design exists to avoid.
    driver.step(&mut run, true).await;
    assert_eq!(
        workspaces.nudges().len(),
        1,
        "one nudge, not one per iteration"
    );

    // Still nothing. The task is failed and, having attempts left in an autonomous run, handed
    // straight to a fresh agent — so it is Running again, but on a second attempt rather than
    // the abandoned first.
    let attempts_before = run.graph.get(task).unwrap().attempts;
    workspaces.set_idle(task, Duration::from_secs(500));
    driver.step(&mut run, true).await;

    assert!(
        run.graph.get(task).unwrap().attempts > attempts_before,
        "prolonged silence must end the attempt rather than leaving it Running forever"
    );
}

/// Tasks a test hands to the driver, as an operator would.
#[derive(Default)]
struct Added(parking_lot::Mutex<Vec<deck_supervisor::guidance::RequestedTask>>);

impl deck_supervisor::guidance::TaskQueue for Added {
    fn drain(&self) -> Vec<deck_supervisor::guidance::RequestedTask> {
        std::mem::take(&mut *self.0.lock())
    }
}

#[tokio::test]
async fn a_task_added_mid_run_is_planned_dispatched_and_verified_like_any_other() {
    // The case: a reviewer finds a real defect whose fix belongs to a task that is already
    // complete. Retrying the review cannot help — nothing it does changes the artifact it judges
    // — and retry/abandon/end-the-run are all the wrong answer. One more small task is the right
    // one, and there was no way to ask for it without restarting the run.
    let root = workdir("added-task");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan("true"), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let added = Added::default();

    let driver = Driver::new(&cfg, &planner, &workspaces).with_added_tasks(&added);
    let mut run = Run::new();
    driver.step(&mut run, true).await;
    let planned = run.graph.tasks().count();

    added
        .0
        .lock()
        .push(deck_supervisor::guidance::RequestedTask {
            title: "Correct the README command".into(),
            role: "developer".into(),
            description: "python is not on PATH; the README should say python3".into(),
            verify_command: "true".into(),
        });

    driver.step(&mut run, true).await;

    assert_eq!(
        run.graph.tasks().count(),
        planned + 1,
        "the task joined the graph"
    );
    let added_id = run
        .titles
        .iter()
        .find(|(_, title)| title.as_str() == "Correct the README command")
        .map(|(id, _)| *id)
        .expect("titled like the operator asked");

    assert_eq!(
        run.roles.get(&added_id).map(String::as_str),
        Some("developer")
    );
    assert!(
        run.contracts
            .get(&added_id)
            .is_some_and(|c| c.has_executable_criterion()),
        "asked for by a human is a reason to exist, not a reason to skip the gate"
    );
    assert!(
        !run.graph.get(added_id).unwrap().objective_gate,
        "an afterthought must not decide whether the run may finish"
    );
}

/// An autonomy mode the test can change underneath a running driver.
struct Switchable(parking_lot::Mutex<Autonomy>);

impl deck_supervisor::autonomy::AutonomySource for Switchable {
    fn current(&self) -> Autonomy {
        *self.0.lock()
    }
}

#[tokio::test]
async fn changing_autonomy_mid_run_takes_effect_on_the_next_dispatch() {
    // Reported as "the autonomy doesn't work". The mode was fixed in RunConfig at run start, so
    // the control was inert for the life of a run — which is exactly when someone decides they
    // would rather approve each agent, or stop being asked. Both places the mode is read are
    // consulted at the moment they matter, so nothing required it to be constant.
    let root = workdir("live-autonomy");
    let cfg = config(root.clone(), Autonomy::Autonomous, "true");
    let planner = ScriptedPlanner::new();
    planner.push(
        json!({
            "tasks": (0..2).map(|i| json!({
                "tmp_id": format!("t{i}"), "title": format!("Task {i}"), "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [{
                        "id": format!("c{i}"), "text": "it works",
                        "verify": { "type": "command", "cmd": "true", "expect_exit_zero": true }
                    }],
                    "constraints": [], "deliverables": [], "definition_of_done": "done"
                }
            })).collect::<Vec<_>>(),
            "edges": [], "reasoning": "two independent tasks"
        }),
        0.10,
    );

    let workspaces = FakeWorkspaces::new(root);
    // Starts autonomous, so the first pass dispatches without asking.
    let live = Switchable(parking_lot::Mutex::new(Autonomy::Autonomous));
    let driver = Driver::new(&cfg, &planner, &workspaces).with_autonomy(&live);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    assert_eq!(dispatched.len(), 2, "autonomous starts its own agents");
    assert!(run.awaiting_approval.is_empty());

    // The operator decides they want to approve each one from here.
    *live.0.lock() = Autonomy::Manual;

    // A fresh task arrives, and this one must wait even though the run began autonomous.
    let held = TaskId::new();
    run.graph
        .apply(deck_supervisor::graph::Mutation {
            add_tasks: vec![deck_core::domain::task::TaskState {
                id: held,
                assignee: Some(cfg.team[0].agent_id),
                status: TaskStatus::Assigned,
                ..deck_core::domain::task::TaskState::new(held)
            }],
            add_edges: Vec::new(),
        })
        .unwrap();
    run.roles.insert(held, "developer".into());
    run.titles.insert(held, "Later work".into());

    driver.step(&mut run, true).await;

    assert!(
        run.awaiting_approval.contains(&held),
        "the mode change should hold the next dispatch, not wait for the next run"
    );
}
