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
