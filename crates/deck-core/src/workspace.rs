//! Per-agent workspaces.
//!
//! An agent's containment root is its own worktree, so its policy and broker must be
//! per-agent too. A single app-wide broker would make every agent's boundary the union of all
//! their worktrees — which is not isolation, it is the appearance of it.
//!
//! This type is the single place where "which agent, which task, which directory, which
//! policy" are tied together, so a caller cannot accidentally spawn an agent with someone
//! else's containment root.

use crate::domain::ids::SessionId;
use crate::domain::ids::{AgentId, TaskId};
use crate::git::worktree::{WorktreeInfo, WorktreeSpec};
use crate::git::{Result as GitResult, WorktreeManager};
use crate::permission::{worker_defaults, EffectivePolicy, PermissionBroker, PolicyLayer};
use crate::runtime::claude_code::actor::SpawnOptions;
use crate::runtime::claude_code::argv::{PermissionMode, SessionConfig};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Everything an agent needs to run one task in isolation.
pub struct AgentWorkspace {
    pub agent_id: AgentId,
    pub agent_slug: String,
    pub task_id: TaskId,
    pub worktree: WorktreeInfo,
    /// Rooted at `worktree.path`, so this agent's boundary is its own directory and nobody
    /// else's.
    pub broker: Arc<PermissionBroker>,
    /// Role prompt appended to the CLI's default system prompt.
    pub system_prompt: Option<String>,
    pub model: Option<String>,
}

impl AgentWorkspace {
    /// Builds spawn options that cannot disagree with the workspace.
    ///
    /// cwd is the worktree and no `--add-dir` is passed, which is what makes the CLI itself
    /// route out-of-tree writes into the permission pipeline rather than relying on our policy
    /// alone.
    pub fn spawn_options(&self, session_id: SessionId) -> SpawnOptions {
        let mut config = SessionConfig::new(session_id, self.worktree.path.clone());
        config.permission_mode = PermissionMode::AcceptEdits;
        config.model = self.model.clone();
        config.system_prompt_append = self.system_prompt.clone();
        config.tools = vec![
            "Read".into(),
            "Write".into(),
            "Edit".into(),
            "Glob".into(),
            "Grep".into(),
            "Bash".into(),
        ];

        let mut opts = SpawnOptions::new(config);
        opts.broker = Some(self.broker.clone());
        opts
    }

    pub fn worktree_path(&self) -> &Path {
        &self.worktree.path
    }
}

/// Creates and tracks agent workspaces for one project.
pub struct WorkspaceRegistry {
    repo: PathBuf,
    worktrees: WorktreeManager,
    /// Keyed by task, because a worktree is task-scoped: an agent working two tasks gets two
    /// trees, and a retry of the same task reuses one.
    by_task: DashMap<TaskId, Arc<AgentWorkspace>>,
}

impl WorkspaceRegistry {
    pub fn new(repo: PathBuf) -> Self {
        Self {
            repo,
            worktrees: WorktreeManager::new(),
            by_task: DashMap::new(),
        }
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }

    pub fn worktrees(&self) -> &WorktreeManager {
        &self.worktrees
    }

    /// Prepares an agent to work on a task, creating the worktree if needed.
    ///
    /// Idempotent per task so a retry or crash recovery reuses the existing tree and its
    /// uncommitted work rather than starting over.
    pub async fn prepare(&self, request: PrepareRequest<'_>) -> GitResult<Arc<AgentWorkspace>> {
        if let Some(existing) = self.by_task.get(&request.task_id) {
            return Ok(existing.clone());
        }

        let worktree = self
            .worktrees
            .ensure(
                &self.repo,
                &WorktreeSpec {
                    agent_slug: request.agent_slug.to_string(),
                    task_short_id: short_id(request.task_id),
                    task_title: request.task_title.to_string(),
                    base_ref: request.base_ref.to_string(),
                },
            )
            .await?;

        // The policy root is the canonicalized worktree, so containment comparisons are exact.
        // On macOS /var vs /private/var would otherwise never match and every write would
        // escalate.
        let root = worktree
            .path
            .canonicalize()
            .unwrap_or_else(|_| worktree.path.clone());

        let mut layers = vec![worker_defaults()];
        layers.extend(request.extra_layers.iter().cloned());
        let policy = EffectivePolicy::resolve(root, &layers);

        let workspace = Arc::new(AgentWorkspace {
            agent_id: request.agent_id,
            agent_slug: request.agent_slug.to_string(),
            task_id: request.task_id,
            worktree,
            broker: Arc::new(PermissionBroker::new(policy)),
            system_prompt: request.system_prompt.map(str::to_string),
            model: request.model.map(str::to_string),
        });

        self.by_task.insert(request.task_id, workspace.clone());
        Ok(workspace)
    }

    pub fn get(&self, task_id: TaskId) -> Option<Arc<AgentWorkspace>> {
        self.by_task.get(&task_id).map(|e| e.clone())
    }

    /// Total prompts awaiting an answer across every agent. Drives the escalation badge, which
    /// has to be app-wide even though brokers are per-agent.
    pub fn pending_permissions(&self) -> usize {
        self.by_task.iter().map(|e| e.broker.pending_count()).sum()
    }

    /// Resolves a permission request without the caller needing to know which agent owns it.
    ///
    /// Request ids come from the CLI and are unique per session, so a search is unambiguous.
    /// The alternative — making the UI track request-to-agent mapping — would put safety-
    /// critical bookkeeping in the frontend.
    pub fn resolve_permission(
        &self,
        request_id: &str,
        resolution: crate::permission::Resolution,
    ) -> Result<(), crate::permission::broker::BrokerError> {
        for entry in self.by_task.iter() {
            if entry.broker.is_pending(request_id) {
                return entry.broker.resolve(request_id, resolution);
            }
        }
        Err(crate::permission::broker::BrokerError::Unknown(
            request_id.to_string(),
        ))
    }

    /// Forgets a task's workspace. Does **not** remove the worktree: dropping our record must
    /// never be what deletes an agent's uncommitted work.
    pub fn forget(&self, task_id: TaskId) -> Option<Arc<AgentWorkspace>> {
        self.by_task.remove(&task_id).map(|(_, w)| w)
    }

    pub fn active_task_ids(&self) -> Vec<TaskId> {
        self.by_task.iter().map(|e| *e.key()).collect()
    }
}

pub struct PrepareRequest<'a> {
    pub agent_id: AgentId,
    pub agent_slug: &'a str,
    pub task_id: TaskId,
    pub task_title: &'a str,
    pub base_ref: &'a str,
    pub system_prompt: Option<&'a str>,
    pub model: Option<&'a str>,
    /// Layers applied on top of the worker defaults. Denials still union, so a caller cannot
    /// widen the boundary by passing a permissive layer.
    pub extra_layers: Vec<PolicyLayer>,
}

/// Short, filesystem- and branch-safe form of a task id.
fn short_id(task_id: TaskId) -> String {
    task_id.to_string().chars().take(8).collect()
}
