//! The run, its tasks, and the decisions that produced them.
//!
//! The supervisor is deliberately built without a long-lived model session — its memory is
//! supposed to *be* the database. That claim only holds if the graph and the decision log
//! actually reach disk. Until they do, closing the app loses the entire plan: what was asked
//! for, what was attempted, which agent did what, and why the supervisor chose any of it.
//!
//! What is written here is also the regression corpus the design leans on. Every stage is a
//! pure function of a snapshot plus a decision, so a recorded decision log can be replayed and
//! asserted to produce identical mutations. A log that only existed in memory could never serve
//! that purpose.

use super::{Store, StoreError};
use crate::domain::ids::{AgentId, TaskId};

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub run_id: String,
    pub project_id: String,
    pub objective: String,
    /// The loop phase, lowercased to match the schema's allowed values.
    pub status: String,
    pub autonomy: String,
    pub iteration: u32,
    pub budget_usd: Option<f64>,
    pub spent_usd: f64,
    pub ended: bool,
}

#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: TaskId,
    pub title: String,
    pub status: String,
    pub role: String,
    pub assignee: Option<AgentId>,
    pub contract_json: String,
    pub objective_gate: bool,
    pub attempts: u32,
    pub review_rounds: u32,
    pub failure_reason: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DecisionRecord {
    pub iteration: u32,
    pub stage: String,
    pub kind: String,
    pub decided_by: String,
    pub rationale: String,
    pub repair_count: u32,
    pub cost_usd: Option<f64>,
}

/// Writes the run's current state.
///
/// Upsert rather than insert: the run row is rewritten after every iteration, and a run that
/// could only be recorded once would freeze at iteration zero — which is exactly the state a
/// restart would then restore.
pub async fn upsert_run(store: &Store, run: &RunRecord) -> Result<(), StoreError> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO supervisor_runs
             (id, project_id, objective, status, autonomy, iteration, budget_usd, spent_usd,
              started_at, updated_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET
             status = excluded.status, iteration = excluded.iteration,
             spent_usd = excluded.spent_usd, updated_at = excluded.updated_at,
             ended_at = excluded.ended_at",
    )
    .bind(&run.run_id)
    .bind(&run.project_id)
    .bind(&run.objective)
    .bind(&run.status)
    .bind(&run.autonomy)
    .bind(run.iteration as i64)
    .bind(run.budget_usd)
    .bind(run.spent_usd)
    .bind(now)
    .bind(run.ended.then_some(now))
    .execute(store.writer())
    .await?;
    Ok(())
}

/// Writes the whole task graph as it currently stands.
///
/// One statement per task in a single transaction. Task state changes together within an
/// iteration — a task moving to `running` and another to `blocked` are one decision's effects —
/// so a partial write would persist a graph the supervisor never actually held.
pub async fn upsert_tasks(
    store: &Store,
    run_id: &str,
    project_id: &str,
    tasks: &[TaskRecord],
) -> Result<(), StoreError> {
    let now = now_ms();
    let mut tx = store.writer().begin().await?;

    for task in tasks {
        sqlx::query(
            "INSERT INTO tasks
                 (id, project_id, supervisor_run_id, title, description, status,
                  assignee_agent_id, contract_json, objective_gate, attempts, review_rounds,
                  failure_reason, branch, created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT (id) DO UPDATE SET
                 status = excluded.status, assignee_agent_id = excluded.assignee_agent_id,
                 attempts = excluded.attempts, review_rounds = excluded.review_rounds,
                 failure_reason = excluded.failure_reason, branch = excluded.branch,
                 contract_json = excluded.contract_json, completed_at = excluded.completed_at",
        )
        .bind(task.task_id.to_string())
        .bind(project_id)
        .bind(run_id)
        .bind(&task.title)
        // The role, kept where a human reading the table can see who the work was meant for.
        .bind(&task.role)
        .bind(&task.status)
        .bind(task.assignee.map(|a| a.to_string()))
        .bind(&task.contract_json)
        .bind(task.objective_gate as i64)
        .bind(task.attempts as i64)
        .bind(task.review_rounds as i64)
        .bind(&task.failure_reason)
        .bind(&task.branch)
        .bind(now)
        .bind((task.status == "completed").then_some(now))
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Appends decisions the log has not recorded yet.
///
/// The caller passes only the new tail. Rewriting the whole log each iteration would grow
/// quadratically over a long run, and the log is append-only by nature — a decision, once made,
/// never changes.
pub async fn append_decisions(
    store: &Store,
    run_id: &str,
    decisions: &[DecisionRecord],
) -> Result<(), StoreError> {
    if decisions.is_empty() {
        return Ok(());
    }

    let now = now_ms();
    let mut tx = store.writer().begin().await?;

    for decision in decisions {
        sqlx::query(
            "INSERT INTO decisions
                 (id, supervisor_run_id, iteration, at, stage, kind, decided_by, status,
                  repair_count, rationale, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'applied', ?8, ?9, ?10)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(run_id)
        .bind(decision.iteration as i64)
        .bind(now)
        .bind(&decision.stage)
        .bind(&decision.kind)
        .bind(&decision.decided_by)
        .bind(decision.repair_count as i64)
        .bind(&decision.rationale)
        .bind(decision.cost_usd)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// A run recovered from disk, enough to show the operator what the last session was doing.
#[derive(Debug, Clone, PartialEq)]
pub struct PastRun {
    pub run_id: String,
    pub objective: String,
    pub status: String,
    pub autonomy: String,
    pub iteration: u32,
    pub spent_usd: f64,
    pub task_count: i64,
    pub decision_count: i64,
}

/// Recent runs, newest first.
///
/// The objective is the reusable part. A run cannot literally be resumed — its agents are gone
/// and its loop is not running — so what history offers is the ability to pick up where you left
/// off by starting the same objective again, with the previous attempt's outcome visible.
pub async fn recent(
    store: &Store,
    project_id: &str,
    limit: i64,
) -> Result<Vec<PastRun>, StoreError> {
    let rows: Vec<(String, String, String, String, i64, f64)> = sqlx::query_as(
        "SELECT id, objective, status, autonomy, iteration, spent_usd
         FROM supervisor_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT ?2",
    )
    .bind(project_id)
    .bind(limit)
    .fetch_all(store.reader())
    .await?;

    let mut out = Vec::new();
    for (run_id, objective, status, autonomy, iteration, spent_usd) in rows {
        let (task_count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE supervisor_run_id = ?1")
                .bind(&run_id)
                .fetch_one(store.reader())
                .await?;
        let (decision_count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM decisions WHERE supervisor_run_id = ?1")
                .bind(&run_id)
                .fetch_one(store.reader())
                .await?;
        out.push(PastRun {
            run_id,
            objective,
            status,
            autonomy,
            iteration: iteration as u32,
            spent_usd,
            task_count,
            decision_count,
        });
    }
    Ok(out)
}

/// The most recent run for a project.
pub async fn last_run(store: &Store, project_id: &str) -> Result<Option<PastRun>, StoreError> {
    let row: Option<(String, String, String, String, i64, f64)> = sqlx::query_as(
        "SELECT id, objective, status, autonomy, iteration, spent_usd
         FROM supervisor_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(store.reader())
    .await?;

    let Some((run_id, objective, status, autonomy, iteration, spent_usd)) = row else {
        return Ok(None);
    };

    let (task_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE supervisor_run_id = ?1")
            .bind(&run_id)
            .fetch_one(store.reader())
            .await?;
    let (decision_count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM decisions WHERE supervisor_run_id = ?1")
            .bind(&run_id)
            .fetch_one(store.reader())
            .await?;

    Ok(Some(PastRun {
        run_id,
        objective,
        status,
        autonomy,
        iteration: iteration as u32,
        spent_usd,
        task_count,
        decision_count,
    }))
}

/// Marks runs a previous launch left mid-flight.
///
/// A run cannot survive the process that was driving it: the loop, the agents and the in-memory
/// graph all died with it. Leaving the row as `monitoring` would show the operator a live run
/// that nothing is advancing, and no amount of waiting would change it.
pub async fn mark_interrupted_on_boot(store: &Store) -> Result<u64, StoreError> {
    let result = sqlx::query(
        "UPDATE supervisor_runs SET status = 'cancelled', ended_at = ?1
         WHERE ended_at IS NULL AND status NOT IN ('completed', 'failed', 'cancelled')",
    )
    .bind(now_ms())
    .execute(store.writer())
    .await?;
    Ok(result.rows_affected())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
