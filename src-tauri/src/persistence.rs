//! Writing the run to disk as it happens.
//!
//! Sits between the supervisor loop, which is synchronous and must not block, and the store,
//! which is async. The observer that drives this runs inside the loop between iterations, so
//! anything slow here would stall the whole team — the write is therefore spawned and its
//! failure logged rather than awaited.
//!
//! That trade is deliberate and worth stating: a failed run write loses the *dashboard's* memory
//! of what happened, not the audit trail. The event log is the durable spine and has its own
//! backpressured writer precisely so that slowing an agent beats losing its history.

use deck_core::store::runs::{self, DecisionRecord, RunRecord, TaskRecord};
use deck_core::store::Store;
use deck_supervisor::driver::Run;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub struct RunWriter {
    store: Store,
    run_id: String,
    project_id: String,
    objective: String,
    autonomy: String,
    budget_usd: f64,
    /// How much of the decision log has already been written. The log is append-only, so only
    /// the tail is new — rewriting all of it each iteration would grow quadratically over a run
    /// long enough to matter.
    written_decisions: Arc<AtomicUsize>,
}

impl RunWriter {
    pub fn new(
        store: Store,
        run_id: String,
        project_id: String,
        objective: String,
        autonomy: String,
        budget_usd: f64,
    ) -> Self {
        Self {
            store,
            run_id,
            project_id,
            objective,
            autonomy,
            budget_usd,
            written_decisions: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Snapshots the run and writes it in the background.
    ///
    /// Takes `&Run` and copies what it needs before returning, because the loop owns the run
    /// mutably and will have moved on by the time the write lands.
    pub fn record(&self, run: &Run) {
        let record = RunRecord {
            run_id: self.run_id.clone(),
            project_id: self.project_id.clone(),
            objective: self.objective.clone(),
            status: phase_to_status(run.state.phase),
            autonomy: self.autonomy.clone(),
            iteration: run.state.iteration,
            budget_usd: Some(self.budget_usd),
            spent_usd: run.state.spent_usd,
            ended: run.state.phase.is_terminal(),
        };

        let tasks: Vec<TaskRecord> = run
            .graph
            .tasks()
            .map(|task| TaskRecord {
                task_id: task.id,
                title: run
                    .titles
                    .get(&task.id)
                    .cloned()
                    .unwrap_or_else(|| task.id.to_string()),
                status: format!("{:?}", task.status).to_lowercase(),
                role: run.roles.get(&task.id).cloned().unwrap_or_default(),
                assignee: task.assignee,
                // The contract is what the task was actually held to. Without it a completed
                // task on disk says it passed but not what it had to pass.
                contract_json: run
                    .contracts
                    .get(&task.id)
                    .and_then(|c| serde_json::to_string(c).ok())
                    .unwrap_or_else(|| "{}".into()),
                objective_gate: task.objective_gate,
                attempts: task.attempts,
                review_rounds: task.review_rounds,
                failure_reason: task.failure_reason.clone(),
                branch: None,
            })
            .collect();

        // Read once and advanced only after a successful write, so a failed write is retried on
        // the next iteration rather than leaving a permanent hole in the log.
        let already = self.written_decisions.load(Ordering::SeqCst);
        let new_decisions: Vec<DecisionRecord> = run
            .log
            .decisions
            .iter()
            .skip(already)
            .map(|d| DecisionRecord {
                iteration: d.iteration,
                stage: d.stage.clone(),
                kind: d.kind.clone(),
                decided_by: format!("{:?}", d.decided_by).to_lowercase(),
                rationale: d.rationale.clone(),
                repair_count: d.repair_count,
                cost_usd: d.cost_usd,
            })
            .collect();
        let total = run.log.decisions.len();

        let store = self.store.clone();
        let run_id = self.run_id.clone();
        let project_id = self.project_id.clone();
        let cursor = self.written_decisions.clone();

        tauri::async_runtime::spawn(async move {
            if let Err(e) = runs::upsert_run(&store, &record).await {
                tracing::error!(%e, "could not persist the run");
                return;
            }
            if let Err(e) = runs::upsert_tasks(&store, &run_id, &project_id, &tasks).await {
                tracing::error!(%e, "could not persist the task graph");
                return;
            }
            match runs::append_decisions(&store, &run_id, &new_decisions).await {
                Ok(()) => {
                    cursor.store(total, Ordering::SeqCst);
                }
                Err(e) => tracing::error!(%e, "could not persist the decision log"),
            }
        });
    }
}

/// Maps a loop phase to the schema's run status.
///
/// Explicit rather than a lowercased debug string: the column has a CHECK constraint, and a
/// phase renamed in Rust would otherwise start silently failing every write at runtime.
fn phase_to_status(phase: deck_supervisor::loop_engine::RunPhase) -> String {
    use deck_supervisor::loop_engine::RunPhase;
    match phase {
        RunPhase::Planning => "planning",
        RunPhase::Dispatching => "dispatching",
        RunPhase::Monitoring => "monitoring",
        RunPhase::Reviewing => "reviewing",
        RunPhase::Replanning => "replanning",
        RunPhase::BlockedOnHuman => "blocked_on_human",
        RunPhase::Completed => "completed",
        RunPhase::Failed => "failed",
        RunPhase::Cancelled => "cancelled",
    }
    .to_string()
}
