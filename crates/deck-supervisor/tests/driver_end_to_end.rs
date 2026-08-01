//! The loop, end to end, driven by scripted model answers.
//!
//! This is the acceptance test for "deterministic controller": the same scripted responses must
//! always produce the same state transitions, and a model that misbehaves must not be able to
//! move the run anywhere it shouldn't go. Running this against a live model would be slow,
//! non-deterministic, and would spend real rate limit to assert things about control flow.

use deck_core::domain::ids::AgentId;
use deck_core::domain::task::TaskStatus;
use deck_supervisor::decision::PlanLimits;
use deck_supervisor::driver::{claim_done, Driver, IterationOutcome, Run, RunConfig, TeamMember};
use deck_supervisor::loop_engine::{RunLimits, RunPhase};
use deck_supervisor::planner::ScriptedPlanner;
use deck_supervisor::workspaces::FakeWorkspaces;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-drv-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn config(root: PathBuf, default_test: &str) -> RunConfig {
    RunConfig {
        objective: "Add a health endpoint".into(),
        team: vec![
            TeamMember {
                agent_id: AgentId::new(),
                role: "developer".into(),
            },
            TeamMember {
                agent_id: AgentId::new(),
                role: "reviewer".into(),
            },
        ],
        default_test_command: default_test.into(),
        verification_root: root,
        limits: RunLimits::default(),
        plan_limits: PlanLimits::default(),
        per_call_budget_usd: 1.0,
        verification_timeout: Duration::from_secs(20),
        // These exercise the pipeline itself, so nothing should be waiting on a human.
        autonomy: deck_supervisor::autonomy::Autonomy::Autonomous,
    }
}

/// A one-task plan whose acceptance criterion is `cmd`.
fn plan_with(cmd: &str) -> serde_json::Value {
    json!({
        "tasks": [{
            "tmp_id": "t1",
            "title": "Implement the endpoint",
            "description": "",
            "role": "developer",
            "objective_gate": true,
            "contract": {
                "version": 1,
                "acceptance_criteria": [{
                    "id": "tests",
                    "text": "tests pass",
                    "verify": { "type": "command", "cmd": cmd, "expect_exit_zero": true }
                }],
                "constraints": [],
                "deliverables": [],
                "definition_of_done": "the endpoint works"
            }
        }],
        "edges": [],
        "reasoning": "single task"
    })
}

#[tokio::test]
async fn a_full_run_reaches_completed_when_verification_passes() {
    let root = workdir("happy");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();
    planner.push(plan_with("true"), 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    // Iteration 1: plan, assign, dispatch.
    let outcome = driver.step(&mut run, true).await;
    let dispatched = match outcome {
        IterationOutcome::Advanced { dispatched } => dispatched,
        other => panic!("expected the pipeline to advance, got {other:?}"),
    };
    assert_eq!(dispatched.len(), 1, "the single task should be dispatched");
    let task_id = dispatched[0];
    assert_eq!(run.graph.get(task_id).unwrap().status, TaskStatus::Running);

    // The agent finishes and claims completion — the only legal completion path.
    assert!(claim_done(&mut run, task_id));
    assert_eq!(run.graph.get(task_id).unwrap().status, TaskStatus::Review);

    // Iteration 2: verification runs and, with no judgment criteria, completes the task.
    driver.step(&mut run, true).await;
    assert_eq!(
        run.graph.get(task_id).unwrap().status,
        TaskStatus::Completed
    );

    // Iteration 3: the sweep sees every gate satisfied.
    let outcome = driver.step(&mut run, true).await;
    assert_eq!(outcome, IterationOutcome::Terminal(RunPhase::Completed));
}

#[tokio::test]
async fn a_red_test_fails_the_task_without_the_reviewer_being_called() {
    // The central anti-false-success property, asserted at the loop level: the model is never
    // consulted about work that provably does not pass.
    let root = workdir("redtest");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();
    planner.push(plan_with("exit 1"), 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();

    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task_id = dispatched[0];
    claim_done(&mut run, task_id);

    let calls_before = planner.call_count();
    driver.step(&mut run, true).await;

    assert_eq!(
        planner.call_count(),
        calls_before,
        "the reviewer must not be consulted about work that failed verification"
    );
    assert_ne!(
        run.graph.get(task_id).unwrap().status,
        TaskStatus::Completed,
        "a red test must not complete the task"
    );
    assert_eq!(
        run.graph.get(task_id).unwrap().review_rounds,
        1,
        "a rejected verification counts as a review round"
    );
}

#[tokio::test]
async fn a_plan_that_fails_validation_twice_escalates_instead_of_guessing() {
    // There is no deterministic fallback for planning — code cannot invent a decomposition — so
    // escalating is the honest outcome rather than proceeding with something wrong.
    let root = workdir("badplan");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();

    // Both attempts name a role that does not exist.
    let bad = json!({
        "tasks": [{
            "tmp_id": "t1", "title": "x", "role": "database-engineer",
            "objective_gate": true, "description": "", "contract": {}
        }],
        "edges": [], "reasoning": ""
    });
    planner.push(bad.clone(), 0.10);
    planner.push(bad, 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    assert_eq!(run.state.phase, RunPhase::BlockedOnHuman);
    assert_eq!(
        planner.call_count(),
        2,
        "exactly one repair round-trip, never a loop"
    );
    assert!(run.graph.is_empty(), "no tasks from a rejected plan");
}

#[tokio::test]
async fn a_plan_is_repaired_on_the_second_attempt_when_the_model_corrects_itself() {
    let root = workdir("repair");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();

    planner.push(
        json!({
            "tasks": [{
                "tmp_id": "t1", "title": "x", "role": "developer",
                "objective_gate": false, "description": "", "contract": {}
            }],
            "edges": [], "reasoning": ""
        }),
        0.10,
    );
    planner.push(plan_with("true"), 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    assert_eq!(planner.call_count(), 2);
    assert_eq!(run.graph.len(), 1, "the corrected plan should be applied");

    let decision = run
        .log
        .decisions
        .iter()
        .find(|d| d.kind == "decompose_objective")
        .expect("the plan decision should be logged");
    assert_eq!(
        decision.repair_count, 1,
        "the log must show it needed correcting"
    );
    assert!(!decision.validation_errors.is_empty());
}

#[tokio::test]
async fn the_repair_prompt_tells_the_model_what_was_wrong() {
    // A retry that just says "try again" wastes the round-trip.
    let root = workdir("prompt");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();
    planner.push(
        json!({
            "tasks": [{
                "tmp_id": "t1", "title": "x", "role": "nonexistent-role",
                "objective_gate": true, "description": "", "contract": {}
            }],
            "edges": [], "reasoning": ""
        }),
        0.10,
    );
    planner.push(plan_with("true"), 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    Driver::new(&cfg, &planner, &workspaces)
        .step(&mut Run::new(), true)
        .await;

    let calls = planner.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[1].prompt.contains("nonexistent-role"),
        "the retry must name the offending role: {}",
        calls[1].prompt
    );
    assert!(calls[1].prompt.contains("rejected"));
}

#[tokio::test]
async fn a_contract_without_executable_verification_is_repaired_before_the_task_runs() {
    // The planner may omit verification; the supervisor may not accept a contract that cannot
    // be checked. The injected command then genuinely gates.
    let root = workdir("vacuous");
    let cfg = config(root, "exit 1");
    let planner = ScriptedPlanner::new();
    planner.push(
        json!({
            "tasks": [{
                "tmp_id": "t1", "title": "x", "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [{
                        "id": "vibes", "text": "looks good",
                        "verify": { "type": "judgment", "rubric": "good?" }
                    }],
                    "constraints": [], "deliverables": [], "definition_of_done": "done"
                }
            }],
            "edges": [], "reasoning": ""
        }),
        0.10,
    );

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };

    let task_id = dispatched[0];
    let contract = run.contracts.get(&task_id).unwrap();
    assert!(
        contract.has_executable_criterion(),
        "a judgment-only contract must be repaired"
    );

    // The injected command fails, so the task must not complete.
    claim_done(&mut run, task_id);
    driver.step(&mut run, true).await;
    assert_ne!(
        run.graph.get(task_id).unwrap().status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn a_self_contradictory_review_escalates_rather_than_being_applied() {
    // A verdict that passes while marking a criterion unmet is not evidence of anything, so it
    // must not move the task in either direction.
    let root = workdir("contradiction");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();

    planner.push(
        json!({
            "tasks": [{
                "tmp_id": "t1", "title": "x", "role": "developer",
                "objective_gate": true, "description": "",
                "contract": {
                    "version": 1,
                    "acceptance_criteria": [
                        { "id": "tests", "text": "tests pass",
                          "verify": { "type": "command", "cmd": "true", "expect_exit_zero": true } },
                        { "id": "clear", "text": "the code is clear",
                          "verify": { "type": "judgment", "rubric": "clear?" } }
                    ],
                    "constraints": [], "deliverables": [], "definition_of_done": "done"
                }
            }],
            "edges": [], "reasoning": ""
        }),
        0.10,
    );
    // Reviewer passes while marking a criterion unmet.
    planner.push(
        json!({
            "verdict": "pass",
            "criteria": [{ "id": "clear", "met": false, "evidence": "" }],
            "blocking_findings": [],
            "summary": "looks fine to me"
        }),
        0.05,
    );

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task_id = dispatched[0];
    claim_done(&mut run, task_id);

    // Verify passes but leaves a judgment criterion, so the reviewer is consulted.
    driver.step(&mut run, true).await;
    // Judge runs on the following iteration.
    driver.step(&mut run, true).await;

    assert_ne!(
        run.graph.get(task_id).unwrap().status,
        TaskStatus::Completed,
        "a contradictory verdict must not complete the task"
    );
    assert!(run.state.open_escalations > 0, "a human should be asked");
}

#[tokio::test]
async fn cost_accumulates_across_calls_and_stops_the_run_at_the_ceiling() {
    // The CLI reports cost per turn, so the run total must sum rather than track the latest.
    let root = workdir("budget");
    let mut cfg = config(root, "true");
    cfg.limits = RunLimits {
        max_cost_usd: 0.05,
        ..RunLimits::default()
    };

    let planner = ScriptedPlanner::new();
    planner.push(plan_with("true"), 0.10); // already over the ceiling

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    assert!(run.state.spent_usd >= 0.10);
    let outcome = driver.step(&mut run, true).await;

    // Stops spending, but stays alive to be answered. Returning this as terminal used to make
    // the loop exit, so "raise the cap and carry on" was not actually reachable — the run was
    // already gone by the time anyone read the message asking them to decide.
    assert_eq!(
        outcome,
        IterationOutcome::Idle,
        "an over-budget run must stop spending without ending"
    );
    assert_eq!(
        run.state.phase,
        RunPhase::BlockedOnHuman,
        "and it must say that it is waiting on a person"
    );
}

#[tokio::test]
async fn a_planner_error_escalates_rather_than_crashing_the_run() {
    let root = workdir("plannererr");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();
    planner.push_error("connection reset");
    planner.push_error("connection reset again");

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    assert_eq!(run.state.phase, RunPhase::BlockedOnHuman);
}

#[tokio::test]
async fn an_agent_that_never_claims_done_does_not_complete_its_task() {
    // claim_task_done is the only legal completion path. A session that just stops has failed.
    let root = workdir("noclaim");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();
    planner.push(plan_with("true"), 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
        panic!("expected advance");
    };
    let task_id = dispatched[0];

    // No claim_done. Verification has nothing queued.
    driver.step(&mut run, true).await;

    assert_eq!(
        run.graph.get(task_id).unwrap().status,
        TaskStatus::Running,
        "an unclaimed task stays running rather than drifting to complete"
    );
}

#[tokio::test]
async fn claiming_done_from_an_illegal_state_is_refused() {
    let root = workdir("badclaim");
    let cfg = config(root, "true");
    let planner = ScriptedPlanner::new();
    planner.push(plan_with("true"), 0.10);

    let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
    let driver = Driver::new(&cfg, &planner, &workspaces);
    let mut run = Run::new();
    driver.step(&mut run, true).await;

    let task_id = run.graph.tasks().next().unwrap().id;
    assert!(claim_done(&mut run, task_id), "legal from Running");
    assert!(
        !claim_done(&mut run, task_id),
        "a second claim from Review must be refused"
    );
}

#[tokio::test]
async fn the_same_script_always_produces_the_same_outcome() {
    // The determinism claim. If this ever fails, the decision log stops being a usable
    // regression corpus.
    async fn once() -> (RunPhase, usize, Vec<TaskStatus>) {
        let root = workdir("determinism");
        let cfg = config(root, "true");
        let planner = ScriptedPlanner::new();
        planner.push(plan_with("true"), 0.10);

        let workspaces = FakeWorkspaces::new(cfg.verification_root.clone());
        let driver = Driver::new(&cfg, &planner, &workspaces);
        let mut run = Run::new();
        let IterationOutcome::Advanced { dispatched } = driver.step(&mut run, true).await else {
            panic!("expected advance");
        };
        claim_done(&mut run, dispatched[0]);
        driver.step(&mut run, true).await;

        let mut statuses: Vec<TaskStatus> = run.graph.tasks().map(|t| t.status).collect();
        statuses.sort_by_key(|s| format!("{s:?}"));
        (run.state.phase, run.log.decisions.len(), statuses)
    }

    let first = once().await;
    let second = once().await;
    assert_eq!(
        first, second,
        "identical scripts must produce identical runs"
    );
}
