//! The seam between the loop and real agents.
//!
//! The supervisor decides *what* should happen; this trait does it. Keeping it behind a trait is
//! what lets the loop's control flow be tested without spawning processes, creating worktrees or
//! spending rate limit — and those tests are the ones that assert the design's actual claims.
//!
//! It also keeps `deck-supervisor` from depending on process management directly, so the crate
//! stays a decision engine rather than an orchestrator with opinions about `tokio::process`.

use crate::contract::TaskContract;
use async_trait::async_trait;
use deck_core::domain::ids::{AgentId, SessionId, TaskId};
use deck_core::git::{Contribution, IntegrationOutcome};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchRequest {
    pub task_id: TaskId,
    pub agent_id: AgentId,
    pub agent_role: String,
    pub task_title: String,
    /// Rendered by code from the contract, never authored by a model. The worker receives a
    /// deterministic brief so two runs of the same task start identically.
    pub brief: String,
    pub contract: TaskContract,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchedAgent {
    pub session_id: SessionId,
    /// The agent's containment root. Verification runs here, so it must be the tree the agent
    /// actually worked in — not a shared directory.
    pub worktree: PathBuf,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DispatchError {
    #[error("could not prepare a worktree for task {task_id}: {detail}")]
    WorkspaceUnavailable { task_id: TaskId, detail: String },
    #[error("could not start an agent for task {task_id}: {detail}")]
    SpawnFailed { task_id: TaskId, detail: String },
}

/// Prepares isolation and starts agents.
#[async_trait]
pub trait Workspaces: Send + Sync {
    /// Creates the worktree if needed and starts an agent on the task.
    async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchedAgent, DispatchError>;

    /// Where a dispatched task's work lives. `None` before dispatch, or if the worktree is gone —
    /// which the verification gate reports as inconclusive rather than failed, since the work was
    /// never judged.
    fn worktree(&self, task_id: TaskId) -> Option<PathBuf>;

    /// The branch a task's work is on. `None` when it never got a worktree.
    fn branch(&self, task_id: TaskId) -> Option<String>;

    /// Whether the agent working this task is still running.
    ///
    /// `None` means no agent was ever started for it, which is different from one that has died
    /// and must not be treated as a failure.
    fn agent_alive(&self, task_id: TaskId) -> Option<bool>;

    /// Merges the given branches into one tree and runs the project's tests there.
    ///
    /// The last gate before a run may call itself complete. Every task passing alone says
    /// nothing about whether the branches work together, and that gap is exactly where a
    /// multi-agent run fails in a way no per-task check can see.
    async fn integrate(
        &self,
        contributions: &[Contribution],
        test_command: Option<&str>,
        timeout: std::time::Duration,
    ) -> IntegrationOutcome;
}

/// Renders the brief a worker receives.
///
/// Deterministic and code-generated: a model-authored handoff would vary between runs and make
/// the decision log non-replayable. It states the completion protocol explicitly because
/// `claim_task_done` is the only legal completion path, and an agent that simply stops has failed.
pub fn render_brief(title: &str, contract: &TaskContract) -> String {
    let mut brief = format!("# Task\n\n{title}\n");

    if !contract.definition_of_done.trim().is_empty() {
        brief.push_str(&format!(
            "\n## Definition of done\n\n{}\n",
            contract.definition_of_done
        ));
    }

    if !contract.acceptance_criteria.is_empty() {
        brief.push_str("\n## Acceptance criteria\n\n");
        for criterion in &contract.acceptance_criteria {
            brief.push_str(&format!("- [{}] {}\n", criterion.id, criterion.text));
        }
        brief.push_str(
            "\nThese are verified by the supervisor running them itself. Claiming success \
             without them passing will be rejected.\n",
        );
    }

    if !contract.constraints.is_empty() {
        brief.push_str("\n## Constraints\n\n");
        for constraint in &contract.constraints {
            brief.push_str(&format!("- {constraint:?}\n"));
        }
    }

    brief.push_str(
        "\n## Completing\n\n\
         Work only inside your worktree. When you believe the criteria are met, call \
         `claim_task_done`. That is the only way to finish: a session that stops without it is \
         treated as a failure, not a completion.\n",
    );

    brief
}

/// Records what it was asked to do and hands back a directory. Lets the loop be driven end to end
/// without processes or worktrees.
pub struct FakeWorkspaces {
    root: PathBuf,
    dispatched: parking_lot::Mutex<Vec<DispatchRequest>>,
    worktrees: parking_lot::Mutex<std::collections::HashMap<TaskId, PathBuf>>,
    fail_next: parking_lot::Mutex<bool>,
    /// What the next integration should return. Scripted, because a fake has no branches to
    /// merge and the driver's behaviour on each outcome is the thing under test.
    integration: parking_lot::Mutex<Option<IntegrationOutcome>>,
    /// Tasks whose agent a test has declared dead.
    dead: parking_lot::Mutex<Vec<TaskId>>,
}

impl FakeWorkspaces {
    /// `root` stands in for the repository; each task gets a subdirectory beneath it, mirroring
    /// the real per-task isolation so a test can prove verification ran in the right tree.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            dispatched: parking_lot::Mutex::new(Vec::new()),
            worktrees: parking_lot::Mutex::new(Default::default()),
            fail_next: parking_lot::Mutex::new(false),
            integration: parking_lot::Mutex::new(None),
            dead: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Simulates an agent dying without reporting anything.
    pub fn kill_agent(&self, task_id: TaskId) {
        self.dead.lock().push(task_id);
    }

    pub fn set_integration(&self, outcome: IntegrationOutcome) {
        *self.integration.lock() = Some(outcome);
    }

    pub fn fail_next_dispatch(&self) {
        *self.fail_next.lock() = true;
    }

    pub fn dispatched(&self) -> Vec<DispatchRequest> {
        self.dispatched.lock().clone()
    }

    pub fn dispatch_count(&self) -> usize {
        self.dispatched.lock().len()
    }
}

#[async_trait]
impl Workspaces for FakeWorkspaces {
    async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchedAgent, DispatchError> {
        if std::mem::replace(&mut *self.fail_next.lock(), false) {
            return Err(DispatchError::SpawnFailed {
                task_id: request.task_id,
                detail: "injected failure".into(),
            });
        }

        let dir = self
            .root
            .join(format!("task-{}", &request.task_id.to_string()[..8]));
        std::fs::create_dir_all(&dir).map_err(|e| DispatchError::WorkspaceUnavailable {
            task_id: request.task_id,
            detail: e.to_string(),
        })?;

        self.worktrees.lock().insert(request.task_id, dir.clone());
        self.dispatched.lock().push(request);

        Ok(DispatchedAgent {
            session_id: SessionId::new(),
            worktree: dir,
        })
    }

    fn worktree(&self, task_id: TaskId) -> Option<PathBuf> {
        self.worktrees.lock().get(&task_id).cloned()
    }

    fn branch(&self, task_id: TaskId) -> Option<String> {
        self.worktrees
            .lock()
            .contains_key(&task_id)
            .then(|| format!("agentdeck/task-{}", &task_id.to_string()[..8]))
    }

    fn agent_alive(&self, task_id: TaskId) -> Option<bool> {
        // Alive unless a test says otherwise, so scripted runs are not reaped out from under
        // themselves by a fake that has no processes at all.
        if self.dead.lock().contains(&task_id) {
            return Some(false);
        }
        self.worktrees.lock().contains_key(&task_id).then_some(true)
    }

    async fn integrate(
        &self,
        contributions: &[Contribution],
        _test_command: Option<&str>,
        _timeout: std::time::Duration,
    ) -> IntegrationOutcome {
        // Defaults to success, so tests that are not about integration are unaffected by its
        // existence. A test that cares scripts the outcome it wants.
        self.integration
            .lock()
            .clone()
            .unwrap_or_else(|| IntegrationOutcome::Integrated {
                merged: contributions.iter().map(|c| c.branch.clone()).collect(),
            })
    }
}

/// Worker reports waiting to be applied.
///
/// Drained by the driver at the `IngestReports` stage rather than applied as they arrive. Reports
/// come from agent processes at arbitrary moments, and mutating the graph mid-stage would change
/// it underneath code that is reading it.
pub trait ReportQueue: Send + Sync {
    fn drain(
        &self,
    ) -> Vec<(
        deck_core::domain::ids::TaskId,
        deck_core::reporting::WorkerReport,
    )>;
}

/// A queue that never yields anything, for runs with no live agents.
pub struct NoReports;

impl ReportQueue for NoReports {
    fn drain(
        &self,
    ) -> Vec<(
        deck_core::domain::ids::TaskId,
        deck_core::reporting::WorkerReport,
    )> {
        Vec::new()
    }
}
