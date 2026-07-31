//! The supervisor's memory.
//!
//! The design's central claim is that the supervisor has no long-lived model session because its
//! memory *is* the database. These tests are what make that claim checkable: if the graph and
//! the decision log do not survive the process, then closing the app loses what was asked for,
//! what was attempted, and why any of it was chosen — and the decision log can never serve as
//! the replay corpus the pure stages are meant to be tested against.

use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::store::identity;
use deck_core::store::runs::{self, DecisionRecord, RunRecord, TaskRecord};
use deck_core::store::Store;
use std::path::PathBuf;

/// A store with the rows a run hangs off. The agent is registered because a task references
/// its assignee, and production registers the team before assigning anything to it.
async fn store_with_project() -> (Store, String, AgentId) {
    let store = Store::open_in_memory().await.expect("in-memory store");
    let identity = identity::ensure_project(&store, &PathBuf::from("/tmp/repo"))
        .await
        .unwrap();
    let agent = identity::ensure_agent(&store, &identity, AgentId::new(), "developer")
        .await
        .unwrap();
    (store, identity.project_id, agent)
}

fn run_record(run_id: &str, project_id: &str, status: &str, iteration: u32) -> RunRecord {
    RunRecord {
        run_id: run_id.to_string(),
        project_id: project_id.to_string(),
        objective: "Add a health endpoint".into(),
        status: status.to_string(),
        autonomy: "assisted".into(),
        iteration,
        budget_usd: Some(5.0),
        spent_usd: 0.25 * iteration as f64,
        ended: matches!(status, "completed" | "failed" | "cancelled"),
    }
}

fn task_record(title: &str, status: &str, assignee: AgentId) -> TaskRecord {
    TaskRecord {
        task_id: TaskId::new(),
        title: title.into(),
        status: status.into(),
        role: "developer".into(),
        assignee: Some(assignee),
        contract_json: r#"{"version":1}"#.into(),
        objective_gate: true,
        attempts: 1,
        review_rounds: 0,
        failure_reason: None,
        branch: Some("agentdeck/developer/abc".into()),
    }
}

fn decision(iteration: u32, kind: &str) -> DecisionRecord {
    DecisionRecord {
        iteration,
        stage: "Plan".into(),
        kind: kind.into(),
        decided_by: "claude".into(),
        rationale: "because".into(),
        repair_count: 0,
        cost_usd: Some(0.01),
    }
}

#[tokio::test]
async fn a_run_is_rewritten_each_iteration_rather_than_frozen_at_its_first() {
    // The row is written once per iteration. If it could only be inserted, the persisted run
    // would stay at iteration zero — and that is exactly the state a restart would restore.
    let (s, project, _) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();

    runs::upsert_run(&s, &run_record(&run_id, &project, "planning", 0))
        .await
        .unwrap();
    runs::upsert_run(&s, &run_record(&run_id, &project, "monitoring", 4))
        .await
        .unwrap();

    let last = runs::last_run(&s, &project).await.unwrap().unwrap();
    assert_eq!(last.iteration, 4);
    assert_eq!(last.status, "monitoring");
    assert_eq!(last.spent_usd, 1.0);

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM supervisor_runs")
        .fetch_one(s.reader())
        .await
        .unwrap();
    assert_eq!(count, 1, "one run, not one row per iteration");
}

#[tokio::test]
async fn the_task_graph_survives_and_reflects_the_latest_state() {
    let (s, project, agent) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();
    runs::upsert_run(&s, &run_record(&run_id, &project, "monitoring", 1))
        .await
        .unwrap();

    let mut task = task_record("Add the endpoint", "running", agent);
    runs::upsert_tasks(&s, &run_id, &project, std::slice::from_ref(&task))
        .await
        .unwrap();

    task.status = "completed".into();
    task.attempts = 2;
    runs::upsert_tasks(&s, &run_id, &project, &[task.clone()])
        .await
        .unwrap();

    let row: (String, i64, Option<i64>) =
        sqlx::query_as("SELECT status, attempts, completed_at FROM tasks WHERE id = ?1")
            .bind(task.task_id.to_string())
            .fetch_one(s.reader())
            .await
            .unwrap();
    assert_eq!(row.0, "completed");
    assert_eq!(row.1, 2);
    assert!(
        row.2.is_some(),
        "a completed task should record when it finished"
    );

    let last = runs::last_run(&s, &project).await.unwrap().unwrap();
    assert_eq!(last.task_count, 1);
}

#[tokio::test]
async fn the_contract_is_stored_alongside_the_task() {
    // Without it a completed task on disk says it passed but not what it had to pass, which
    // makes the record unauditable after the fact.
    let (s, project, agent) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();
    runs::upsert_run(&s, &run_record(&run_id, &project, "monitoring", 1))
        .await
        .unwrap();

    let task = task_record("Add the endpoint", "completed", agent);
    runs::upsert_tasks(&s, &run_id, &project, std::slice::from_ref(&task))
        .await
        .unwrap();

    let (contract,): (String,) = sqlx::query_as("SELECT contract_json FROM tasks WHERE id = ?1")
        .bind(task.task_id.to_string())
        .fetch_one(s.reader())
        .await
        .unwrap();
    assert!(contract.contains("version"));
}

#[tokio::test]
async fn only_new_decisions_are_appended() {
    // The log is append-only, and rewriting all of it each iteration would grow quadratically
    // over a run long enough for that to matter.
    let (s, project, _) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();
    runs::upsert_run(&s, &run_record(&run_id, &project, "monitoring", 1))
        .await
        .unwrap();

    runs::append_decisions(&s, &run_id, &[decision(0, "plan"), decision(0, "assign")])
        .await
        .unwrap();
    runs::append_decisions(&s, &run_id, &[decision(1, "dispatch")])
        .await
        .unwrap();

    let last = runs::last_run(&s, &project).await.unwrap().unwrap();
    assert_eq!(last.decision_count, 3);

    let kinds: Vec<(String,)> = sqlx::query_as(
        "SELECT kind FROM decisions WHERE supervisor_run_id = ?1 ORDER BY iteration",
    )
    .bind(&run_id)
    .fetch_all(s.reader())
    .await
    .unwrap();
    assert_eq!(kinds.len(), 3);
}

#[tokio::test]
async fn appending_nothing_is_not_an_error() {
    // Most iterations produce no new decisions, and an empty append is the common case rather
    // than an edge one.
    let (s, project, _) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();
    runs::upsert_run(&s, &run_record(&run_id, &project, "monitoring", 1))
        .await
        .unwrap();

    runs::append_decisions(&s, &run_id, &[]).await.unwrap();
    let last = runs::last_run(&s, &project).await.unwrap().unwrap();
    assert_eq!(last.decision_count, 0);
}

#[tokio::test]
async fn a_run_a_crash_interrupted_is_not_left_looking_live() {
    // A run cannot outlive the process driving it — the loop, the agents and the in-memory graph
    // all died with it. Left as `monitoring` it would show the operator a live run that nothing
    // is advancing, and no amount of waiting would change that.
    let (s, project, _) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();
    runs::upsert_run(&s, &run_record(&run_id, &project, "monitoring", 3))
        .await
        .unwrap();

    assert_eq!(runs::mark_interrupted_on_boot(&s).await.unwrap(), 1);

    let last = runs::last_run(&s, &project).await.unwrap().unwrap();
    assert_eq!(last.status, "cancelled");
    assert_eq!(last.iteration, 3, "how far it got is still on record");
}

#[tokio::test]
async fn a_run_that_finished_is_left_alone_by_the_boot_sweep() {
    let (s, project, _) = store_with_project().await;
    let run_id = uuid::Uuid::new_v4().to_string();
    runs::upsert_run(&s, &run_record(&run_id, &project, "completed", 7))
        .await
        .unwrap();

    assert_eq!(runs::mark_interrupted_on_boot(&s).await.unwrap(), 0);
    assert_eq!(
        runs::last_run(&s, &project).await.unwrap().unwrap().status,
        "completed"
    );
}

#[tokio::test]
async fn a_project_with_no_runs_reports_none_rather_than_failing() {
    let (s, project, _) = store_with_project().await;
    assert_eq!(runs::last_run(&s, &project).await.unwrap(), None);
}
