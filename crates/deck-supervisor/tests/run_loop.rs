//! Event-driven loop behaviour.
//!
//! The properties worth testing are about *when the loop spends money*: a bare tick must not
//! provoke an iteration, agent activity must, and a burst of activity must coalesce into one.

use deck_core::domain::event::{AgentEvent, ExitReason};
use deck_core::domain::ids::AgentId;
use deck_supervisor::decision::PlanLimits;
use deck_supervisor::driver::{Driver, Run, RunConfig, TeamMember};
use deck_supervisor::loop_engine::{RunLimits, RunPhase, Trigger};
use deck_supervisor::planner::ScriptedPlanner;
use deck_supervisor::run_loop::{forward_event, trigger_for, LoopExit, RunLoop};
use deck_supervisor::workspaces::FakeWorkspaces;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-rl-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn config(root: PathBuf) -> RunConfig {
    RunConfig {
        objective: "Ship it".into(),
        team: vec![TeamMember {
            agent_id: AgentId::new(),
            role: "developer".into(),
        }],
        default_test_command: "true".into(),
        verification_root: root,
        limits: RunLimits::default(),
        plan_limits: PlanLimits::default(),
        per_call_budget_usd: 1.0,
        verification_timeout: Duration::from_secs(10),
        // These exercise the pipeline itself, so nothing should be waiting on a human.
        autonomy: deck_supervisor::autonomy::Autonomy::Autonomous,
    }
}

fn one_task_plan() -> serde_json::Value {
    json!({
        "tasks": [{
            "tmp_id": "t1", "title": "Do the thing", "role": "developer",
            "objective_gate": true, "description": "",
            "contract": {
                "version": 1,
                "acceptance_criteria": [{
                    "id": "tests", "text": "tests pass",
                    "verify": { "type": "command", "cmd": "true", "expect_exit_zero": true }
                }],
                "constraints": [], "deliverables": [], "definition_of_done": "done"
            }
        }],
        "edges": [], "reasoning": ""
    })
}

// ---------------------------------------------------------------------------
// Event mapping
// ---------------------------------------------------------------------------

#[test]
fn most_events_do_not_wake_the_loop() {
    // Waking on transcript detail would make the loop as expensive as polling, which is the whole
    // thing the tick/sweep split exists to avoid.
    for event in [
        AgentEvent::TurnStarted,
        AgentEvent::Message {
            role: "assistant".into(),
            content: serde_json::Value::Null,
        },
        AgentEvent::ToolCall {
            tool_use_id: "t".into(),
            tool: "Write".into(),
            input: serde_json::Value::Null,
        },
        AgentEvent::SessionReady {
            cwd: "/tmp".into(),
            model: None,
            tools: vec![],
        },
    ] {
        assert_eq!(
            trigger_for(&event),
            None,
            "{event:?} should not wake the loop"
        );
    }
}

#[test]
fn agent_activity_that_changes_the_supervisors_options_does_wake_it() {
    assert_eq!(
        trigger_for(&AgentEvent::TurnComplete {
            subtype: "success".into(),
            is_error: false,
            cost_usd: Some(0.1),
            structured_output: None,
        }),
        Some(Trigger::ReportReceived)
    );
    assert_eq!(
        trigger_for(&AgentEvent::SessionExited {
            reason: ExitReason::Clean
        }),
        Some(Trigger::SessionExited)
    );
    assert_eq!(
        trigger_for(&AgentEvent::PermissionResolved {
            request_id: "r".into(),
            allowed: true,
        }),
        Some(Trigger::HumanAnswered)
    );
}

#[test]
fn a_force_killed_session_does_not_wake_the_loop() {
    // A user-initiated kill is handled by the cancel path. Treating it as ordinary agent activity
    // would make the loop try to replan work the operator just stopped.
    assert_eq!(
        trigger_for(&AgentEvent::SessionExited {
            reason: ExitReason::Killed
        }),
        None
    );
}

#[tokio::test]
async fn a_full_trigger_channel_drops_a_wakeup_rather_than_stalling_the_bus() {
    // Losing a trigger is safe: the tick picks the work up. Backpressuring the event bus would
    // slow every agent, which is far worse.
    let (tx, _rx) = tokio::sync::mpsc::channel::<Trigger>(1);
    let event = AgentEvent::SessionExited {
        reason: ExitReason::Clean,
    };

    assert!(forward_event(&tx, &event), "the first should be accepted");
    assert!(
        !forward_event(&tx, &event),
        "the second should be dropped, not block"
    );
}

// ---------------------------------------------------------------------------
// Loop behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_loop_plans_immediately_rather_than_waiting_for_the_first_tick() {
    // A fresh run has an empty graph and obvious work to do; waiting a tick would add latency for
    // no benefit.
    let root = workdir("immediate");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let mut run = Run::new();

    // Nothing sends a trigger; closing the channel is what ends the loop.
    drop(tx);
    let exit = tokio::time::timeout(
        Duration::from_secs(10),
        RunLoop::new(rx)
            .with_tick(Duration::from_millis(50))
            .run(&driver, &mut run),
    )
    .await
    .expect("the loop should not hang");

    assert_eq!(exit, LoopExit::ChannelClosed);
    assert_eq!(run.graph.len(), 1, "planning should have happened at once");
    assert_eq!(planner.call_count(), 1);
}

#[tokio::test]
async fn ticks_alone_do_not_provoke_further_model_calls() {
    // The core cost property, asserted at the loop level: with the plan already made and an agent
    // working, repeated ticks must not spend anything.
    let root = workdir("ticks");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let mut run = Run::new();

    // Let several ticks elapse before ending the loop.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(tx);
    });

    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        RunLoop::new(rx)
            .with_tick(Duration::from_millis(40))
            .run(&driver, &mut run),
    )
    .await
    .expect("no hang");

    assert_eq!(
        planner.call_count(),
        1,
        "roughly ten ticks passed and only the initial plan cost anything"
    );
}

#[tokio::test]
async fn the_loop_exits_immediately_on_cancel() {
    // An operator asking to stop must take effect at once, not at the next tick.
    let root = workdir("cancel");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let mut run = Run::new();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = tx.send(Trigger::CancelRequested).await;
    });

    let started = std::time::Instant::now();
    let exit = tokio::time::timeout(
        Duration::from_secs(10),
        // A long tick, so passing this proves cancel did not merely wait for one.
        RunLoop::new(rx)
            .with_tick(Duration::from_secs(30))
            .run(&driver, &mut run),
    )
    .await
    .expect("no hang");

    assert_eq!(exit, LoopExit::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancel should not wait for a tick, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn cancel_is_honoured_even_when_it_arrives_in_a_burst() {
    // Coalescing must not swallow a cancel: it is the one trigger that changes the outcome rather
    // than just scheduling work.
    let root = workdir("burstcancel");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);

    let (tx, rx) = tokio::sync::mpsc::channel(16);
    for _ in 0..5 {
        tx.send(Trigger::ReportReceived).await.unwrap();
    }
    tx.send(Trigger::CancelRequested).await.unwrap();

    let mut run = Run::new();
    let exit = tokio::time::timeout(
        Duration::from_secs(10),
        RunLoop::new(rx)
            .with_tick(Duration::from_secs(30))
            .run(&driver, &mut run),
    )
    .await
    .expect("no hang");

    assert_eq!(exit, LoopExit::Cancelled);
}

#[tokio::test]
async fn a_blocked_run_stays_alive_so_an_answer_can_resume_it() {
    // BlockedOnHuman is not terminal. The point of escalating is that answering resumes the run,
    // so the loop must keep waiting rather than exiting and stranding the work.
    let root = workdir("terminal");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    // Both attempts invalid, so planning escalates and the run blocks.
    let bad = json!({
        "tasks": [{
            "tmp_id": "t1", "title": "x", "role": "nope",
            "objective_gate": true, "description": "", "contract": {}
        }],
        "edges": [], "reasoning": ""
    });
    planner.push(bad.clone(), 0.10);
    planner.push(bad, 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let mut run = Run::new();

    // Nothing answers; closing the channel is the only thing that ends the loop, which is
    // precisely the point — a blocked run waits indefinitely for a human.
    drop(tx);
    let exit = tokio::time::timeout(
        Duration::from_secs(10),
        RunLoop::new(rx)
            .with_tick(Duration::from_millis(40))
            .run(&driver, &mut run),
    )
    .await
    .expect("the loop must not spin on a blocked run");

    assert_eq!(exit, LoopExit::ChannelClosed);
    assert_eq!(
        run.state.phase,
        RunPhase::BlockedOnHuman,
        "the run should still be recorded as waiting on a human"
    );
    assert_eq!(
        planner.call_count(),
        2,
        "a blocked run must not keep re-planning while it waits"
    );
}

#[tokio::test]
async fn a_burst_of_reports_causes_one_iteration_not_one_each() {
    // Five agents finishing together should provoke a single supervisory pass; one each would
    // multiply the cost of a busy moment.
    let root = workdir("coalesce");
    let cfg = config(root.clone());
    let planner = ScriptedPlanner::new();
    planner.push(one_task_plan(), 0.10);
    let workspaces = FakeWorkspaces::new(root);
    let driver = Driver::new(&cfg, &planner, &workspaces);

    let (tx, rx) = tokio::sync::mpsc::channel(32);
    for _ in 0..5 {
        tx.send(Trigger::ReportReceived).await.unwrap();
    }
    drop(tx);

    let mut run = Run::new();
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        RunLoop::new(rx)
            .with_tick(Duration::from_millis(40))
            .run(&driver, &mut run),
    )
    .await
    .expect("no hang");

    // Only the initial plan cost anything; the burst produced no further model calls because
    // there was nothing new to decide.
    assert_eq!(planner.call_count(), 1);
    assert!(
        run.state.iteration <= 3,
        "a burst of five reports should not produce five iterations, got {}",
        run.state.iteration
    );
}
