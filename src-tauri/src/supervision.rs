//! The production `Workspaces` implementation.
//!
//! Lives in the app rather than `deck-supervisor` because it is where the decision engine meets
//! process management: it creates a git worktree, builds spawn options that cannot disagree with
//! that worktree, and starts a real `claude` process.

use dashmap::DashMap;
use deck_core::bus::{Attribution, EventBus};
use deck_core::domain::ids::{SessionId, TaskId};
use deck_core::runtime::claude_code::actor::{spawn_session, SessionCmd, SessionHandle};
use deck_core::workspace::{PrepareRequest, WorkspaceRegistry};
use deck_supervisor::workspaces::{DispatchError, DispatchRequest, DispatchedAgent, Workspaces};
use std::path::PathBuf;
use std::sync::Arc;

pub struct LiveWorkspaces {
    registry: Arc<WorkspaceRegistry>,
    bus: Arc<EventBus>,
    /// Live agents, so a run can be stopped and the UI can attach to a transcript.
    sessions: DashMap<TaskId, Arc<SessionHandle>>,
    /// Base branch new worktrees start from.
    base_ref: String,
    model: Option<String>,
}

impl LiveWorkspaces {
    pub fn new(registry: Arc<WorkspaceRegistry>, bus: Arc<EventBus>, base_ref: String) -> Self {
        Self {
            registry,
            bus,
            sessions: DashMap::new(),
            base_ref,
            model: None,
        }
    }

    /// Force-kills every running agent and reports how many were stopped.
    ///
    /// Force rather than a cooperative stop: a wedged agent may never answer, and an operator
    /// asking to stop must always take effect immediately.
    pub fn kill_all(&self) -> usize {
        let mut stopped = 0;
        for entry in self.sessions.iter() {
            if entry.value().kill_now().is_ok() {
                stopped += 1;
            }
        }
        stopped
    }
}

#[async_trait::async_trait]
impl Workspaces for LiveWorkspaces {
    async fn dispatch(&self, request: DispatchRequest) -> Result<DispatchedAgent, DispatchError> {
        let workspace = self
            .registry
            .prepare(PrepareRequest {
                agent_id: request.agent_id,
                agent_slug: &request.agent_role,
                task_id: request.task_id,
                task_title: &request.task_title,
                base_ref: &self.base_ref,
                // The role prompt; the task brief goes as the first message so it appears in the
                // transcript the operator reads rather than being hidden in the system prompt.
                system_prompt: None,
                model: self.model.as_deref(),
                extra_layers: vec![],
            })
            .await
            .map_err(|e| DispatchError::WorkspaceUnavailable {
                task_id: request.task_id,
                detail: e.to_string(),
            })?;

        let session_id = SessionId::new();
        let mut opts = workspace.spawn_options(session_id);
        opts.attribution = Attribution {
            session_id: Some(session_id),
            agent_id: Some(request.agent_id),
            task_id: Some(request.task_id),
        };

        let (handle, _join) = spawn_session(opts, self.bus.clone()).await.map_err(|e| {
            DispatchError::SpawnFailed {
                task_id: request.task_id,
                detail: e.to_string(),
            }
        })?;

        let handle = Arc::new(handle);
        handle
            .send(SessionCmd::SendText(request.brief))
            .await
            .map_err(|e| DispatchError::SpawnFailed {
                task_id: request.task_id,
                detail: format!("agent started but would not accept its brief: {e}"),
            })?;

        self.sessions.insert(request.task_id, handle);

        Ok(DispatchedAgent {
            session_id,
            worktree: workspace.worktree.path.clone(),
        })
    }

    fn worktree(&self, task_id: TaskId) -> Option<PathBuf> {
        self.registry.get(task_id).map(|w| w.worktree.path.clone())
    }
}

/// Consults Claude via a one-shot CLI invocation per decision.
///
/// Mirrors the runtime constraints proven in the M0 spike: `--verbose` is mandatory with
/// stream-json, settings are pinned so the supervisor never inherits the developer's plugins or
/// hooks, and `--json-schema` does the shape validation before our own ladder runs.
///
/// Tools are disabled outright. The supervisor reasons; it does not act. Anything it needs to know
/// about the repository is assembled into the prompt by code, which is what keeps its inputs
/// inspectable and its decisions replayable.
pub struct CliPlanner {
    program: String,
    cwd: PathBuf,
    model: Option<String>,
}

impl CliPlanner {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            program: "claude".into(),
            cwd,
            model: None,
        }
    }
}

#[async_trait::async_trait]
impl deck_supervisor::planner::Planner for CliPlanner {
    async fn call(
        &self,
        call: deck_supervisor::planner::ModelCall,
    ) -> Result<deck_supervisor::planner::ModelResponse, deck_supervisor::planner::PlannerError>
    {
        use deck_supervisor::planner::{ModelResponse, PlannerError};

        let schema =
            serde_json::to_string(&call.schema).map_err(|e| PlannerError::Failed(e.to_string()))?;

        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.arg("-p")
            .arg(&call.prompt)
            .args(["--output-format", "json"])
            .arg("--verbose")
            .args(["--json-schema", &schema])
            .args(["--max-budget-usd", &call.max_budget_usd.to_string()])
            // No plugins, hooks or MCP servers from the developer's environment.
            .args(["--setting-sources", ""])
            .arg("--strict-mcp-config")
            // The supervisor decides; it never touches the repository itself.
            .args(["--tools", ""])
            .args(["--permission-mode", "default"])
            .current_dir(&self.cwd)
            .kill_on_drop(true);

        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| PlannerError::Failed(e.to_string()))?;

        if !output.status.success() {
            return Err(PlannerError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| PlannerError::Parse(e.to_string()))?;

        // `structured_output` is present only when --json-schema validated successfully. Its
        // absence means the model never produced a conforming answer, which the caller treats as
        // an unusable response rather than guessing at the prose.
        let structured = parsed
            .get("structured_output")
            .cloned()
            .ok_or(PlannerError::NoStructuredOutput)?;

        Ok(ModelResponse {
            structured,
            cost_usd: parsed.get("total_cost_usd").and_then(|c| c.as_f64()),
        })
    }
}
