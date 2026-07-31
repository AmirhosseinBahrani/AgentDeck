//! Dispatch and per-task verification.
//!
//! The property that matters: each task is verified in the tree its own agent worked in. Verifying
//! in a shared directory would test the wrong code, and would most likely *pass* — the dangerous
//! direction, and invisible without a test that puts different content in each tree.

use deck_core::domain::ids::AgentId;
use deck_core::domain::task::TaskStatus;
use deck_supervisor::contract::{Criterion, TaskContract, Verification};
use deck_supervisor::decision::PlanLimits;
use deck_supervisor::driver::{claim_done, Driver, IterationOutcome, Run, RunConfig, TeamMember};
use deck_supervisor::loop_engine::{RunLimits, RunPhase};
use deck_supervisor::planner::ScriptedPlanner;
use deck_supervisor::workspaces::{render_brief, FakeWorkspaces, Workspaces};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-rd-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn config(root: PathBuf) -> RunConfig {
    RunConfig {
        objective: "Ship the feature".into(),
        team: vec![TeamMember {
            agent_id: AgentId::new(),
            role: "developer".into(),
        }],
        default_test_command: "true".into(),
        verification_root: root,
        limits: RunLimits::default(),
        plan_limits: PlanLimits::default(),
        per_call_budget_usd: 1.0,
        verification_timeout: Duration::from_secs(20),
    }
}

/// Two independent tasks, each asserting a file that only its own tree will contain.
fn two_task_plan() -> serde_json::Value {
    json!({
        "tasks": [
            {
                "tmp_id": "a", "title": "Task A", "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [{
                        "id": "a-marker", "text": "A's marker exists",
                        "verify": { "type": "command", "cmd": "test -f a.txt", "expect_exit_zero": true }
                    }],
                    "constraints": [], "deliverables": [], "definition_of_done": "A done"
                }
            },
            {
                "tmp_id": "b", "title": "Task B", "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [{
                        "id": "b-marker", "text": "B's marker exists",
                        "verify": { "type": "command", "cmd": "test -f b.txt", "expect_exit_zero": true }
                    }],
                    "constraints": [], "deliverables": [], "definition_of_done": "B done"
                }
            }
        ],
        "edges": [], "reasoning": "two independent tasks"
    })
}

#[tokio::test]
async fn each_task_is_verified_in_its_own_worktree() {
    // If verification ran in a shared directory, both tasks would see both markers and both would
    // pass — the failure mode this test exists to catch.
    let root = workdir("isolation");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(two_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    assert_eq!(dispatched.len(), 2, "both tasks should be dispatched");

    // Each agent creates only its own marker, in its own tree.
    for id in &dispatched {
        let tree = workspaces.worktree(*id).expect("worktree after dispatch");
        let title = run.titles.get(id).cloned().unwrap_or_default();
        let marker = if title.contains('A') {
            "a.txt"
        } else {
            "b.txt"
        };
        std::fs::write(tree.join(marker), "done\n").unwrap();
        claim_done(&mut run, *id);
    }

    driver.step(&mut run, true).await;

    for id in &dispatched {
        assert_eq!(
            run.graph.get(*id).unwrap().status,
            TaskStatus::Completed,
            "each task should pass against its own tree"
        );
    }
}

#[tokio::test]
async fn a_task_verified_against_the_wrong_tree_would_fail() {
    // The inverse of the test above, proving the isolation assertion is meaningful rather than
    // vacuous: an agent that does not create its marker must not pass.
    let root = workdir("wrongtree");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(two_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };

    // Only the first task does its work; the second creates nothing.
    let first = dispatched[0];
    let tree = workspaces.worktree(first).unwrap();
    let title = run.titles.get(&first).cloned().unwrap_or_default();
    let marker = if title.contains('A') {
        "a.txt"
    } else {
        "b.txt"
    };
    std::fs::write(tree.join(marker), "done\n").unwrap();

    for id in &dispatched {
        claim_done(&mut run, *id);
    }
    driver.step(&mut run, true).await;

    assert_eq!(run.graph.get(first).unwrap().status, TaskStatus::Completed);
    assert_ne!(
        run.graph.get(dispatched[1]).unwrap().status,
        TaskStatus::Completed,
        "a task whose criterion is unmet must not pass just because a sibling did its work"
    );
}

#[tokio::test]
async fn dispatch_records_the_session_so_the_ui_can_attach_to_it() {
    let root = workdir("sessions");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(two_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    let mut run = Run::new();
    let IterationOutcome::Advanced { dispatched } = Driver::new(&cfg, &planner, &workspaces)
        .step(&mut run, true)
        .await
    else {
        panic!("expected advance");
    };

    for id in &dispatched {
        assert!(
            run.sessions.contains_key(id),
            "a dispatched task must have a session recorded"
        );
    }
    let unique: std::collections::HashSet<_> = run.sessions.values().collect();
    assert_eq!(unique.len(), dispatched.len(), "sessions must be distinct");
}

#[tokio::test]
async fn a_dispatch_failure_escalates_rather_than_consuming_a_retry() {
    // A worktree or spawn failure is an environment problem, not the agent's fault. Charging the
    // task an attempt for work that never started would exhaust its budget on infrastructure.
    let root = workdir("dispatchfail");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(two_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    workspaces.fail_next_dispatch();

    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    assert_eq!(run.state.phase, RunPhase::BlockedOnHuman);
    assert!(run.state.open_escalations > 0);

    let charged: Vec<u32> = run.graph.tasks().map(|t| t.attempts).collect();
    assert!(
        charged.iter().all(|a| *a == 0),
        "a failed dispatch must not consume an attempt, got {charged:?}"
    );
}

#[tokio::test]
async fn a_task_is_only_marked_running_once_an_agent_actually_started() {
    // Marking Running on intent rather than on success would consume an attempt for work that
    // never began, and would make the task look active in the UI while nothing was happening.
    let root = workdir("intent");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(two_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    workspaces.fail_next_dispatch();

    let mut run = Run::new();
    Driver::new(&cfg, &planner, &workspaces)
        .step(&mut run, true)
        .await;

    let running = run
        .graph
        .tasks()
        .filter(|t| t.status == TaskStatus::Running)
        .count();
    assert_eq!(
        running, 0,
        "nothing should be Running after a failed dispatch"
    );
}

// ---------------------------------------------------------------------------
// The brief handed to a worker
// ---------------------------------------------------------------------------

#[test]
fn the_brief_states_the_completion_protocol_explicitly() {
    // claim_task_done is the only legal completion path, so a worker that does not know that will
    // stop when it thinks it is finished — and be recorded as having failed.
    let contract = TaskContract {
        version: 1,
        acceptance_criteria: vec![Criterion {
            id: "tests".into(),
            text: "the tests pass".into(),
            verify: Verification::Command {
                cmd: "cargo test".into(),
                cwd_rel: None,
                expect_exit_zero: true,
            },
        }],
        constraints: vec![],
        deliverables: vec![],
        definition_of_done: "the endpoint responds".into(),
    };

    let brief = render_brief("Add a health endpoint", &contract);

    assert!(brief.contains("claim_task_done"));
    assert!(
        brief.contains("treated as a failure"),
        "the consequence of just stopping must be stated: {brief}"
    );
    assert!(
        brief.contains("the tests pass"),
        "criteria should be listed"
    );
    assert!(
        brief.contains("verified by the supervisor running them itself"),
        "the worker should know its claims will be checked, not taken on trust"
    );
    assert!(brief.contains("the endpoint responds"));
}

#[test]
fn the_brief_is_deterministic_so_the_same_task_always_starts_identically() {
    // A model-authored handoff would vary between runs and make the decision log non-replayable.
    let contract = TaskContract {
        version: 1,
        definition_of_done: "done".into(),
        ..Default::default()
    };
    assert_eq!(
        render_brief("Task", &contract),
        render_brief("Task", &contract)
    );
}

#[tokio::test]
async fn the_dispatched_brief_carries_the_task_title_and_contract() {
    let root = workdir("brief");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(two_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);

    Driver::new(&cfg, &planner, &workspaces)
        .step(&mut Run::new(), true)
        .await;

    let requests = workspaces.dispatched();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(request.brief.contains(&request.task_title));
        assert!(
            request.brief.contains("claim_task_done"),
            "every worker must be told the completion protocol"
        );
        assert_eq!(request.agent_role, "developer");
    }
}
