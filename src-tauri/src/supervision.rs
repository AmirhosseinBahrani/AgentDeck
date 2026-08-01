//! The production `Workspaces` implementation.
//!
//! Lives in the app rather than `deck-supervisor` because it is where the decision engine meets
//! process management: it creates a git worktree, builds spawn options that cannot disagree with
//! that worktree, and starts a real `claude` process.

use dashmap::DashMap;
use deck_core::bus::{Attribution, EventBus};
use deck_core::domain::ids::{SessionId, TaskId};
use deck_core::report_server::{ReportServer, ReportSink};
use deck_core::reporting::{mcp_config, socket_path, ReportAck, ReportEnvelope, WorkerReport};
use deck_core::runtime::claude_code::actor::{spawn_session, SessionCmd, SessionHandle};
use deck_core::store::identity::LocalIdentity;
use deck_core::store::processes::{self, BootId, ProcessRecord};
use deck_core::store::{sessions, Store};
use deck_core::workspace::{PrepareRequest, WorkspaceRegistry};
use deck_supervisor::workspaces::{DispatchError, DispatchRequest, DispatchedAgent, Workspaces};

// The planner lives in deck-supervisor: it is a Planner implementation with no Tauri
// dependency, and keeping it there is what lets an integration test drive a real CLI call.
pub use deck_supervisor::cli_planner::CliPlanner;
use std::path::PathBuf;
use std::sync::Arc;

pub struct LiveWorkspaces {
    registry: Arc<WorkspaceRegistry>,
    bus: Arc<EventBus>,
    /// Live agents, so a run can be stopped and the UI can attach to a transcript.
    sessions: DashMap<TaskId, Arc<SessionHandle>>,
    /// One reporting socket per session, held so it lives as long as the agent. Dropping it
    /// removes the socket and the agent's tools stop working mid-task.
    report_servers: DashMap<TaskId, ReportServer>,
    /// Where worker reports go. Set by the app before a run starts.
    sink: Arc<dyn ReportSink>,
    /// The MCP server binary handed to each agent.
    mcp_binary: PathBuf,
    runtime_dir: PathBuf,
    /// Base branch new worktrees start from.
    base_ref: String,
    model: Option<String>,
    /// Durable records of what is running, so a crash leaves evidence rather than orphans.
    store: Store,
    boot: BootId,
    identity: LocalIdentity,
    /// The operator's notes and enabled skills, rendered once per run.
    ///
    /// Snapshotted at run start rather than read per dispatch: agents dispatched by the same run
    /// must be told the same thing, or two workers reach contradictory conclusions about the
    /// project and the difference is invisible in both their transcripts.
    knowledge: Option<String>,
}

impl LiveWorkspaces {
    pub fn new(
        registry: Arc<WorkspaceRegistry>,
        bus: Arc<EventBus>,
        base_ref: String,
        sink: Arc<dyn ReportSink>,
        store: Store,
        boot: BootId,
        identity: LocalIdentity,
    ) -> Self {
        Self {
            registry,
            bus,
            sessions: DashMap::new(),
            report_servers: DashMap::new(),
            sink,
            mcp_binary: mcp_binary_path(),
            runtime_dir: PathBuf::from("/tmp"),
            base_ref,
            model: None,
            store,
            boot,
            identity,
            knowledge: None,
        }
    }

    /// Sets the standing project knowledge every agent this run spawns will be given.
    pub fn with_knowledge(mut self, knowledge: Option<String>) -> Self {
        self.knowledge = knowledge;
        self
    }

    /// Force-kills one agent.
    ///
    /// Force rather than a cooperative stop, and deliberately not routed through the session
    /// actor: the case that most needs killing is an agent whose actor is stuck, so a path that
    /// depended on the actor answering would fail exactly when it is needed.
    ///
    /// The worktree, its branch and everything already persisted survive. Killing is an operator
    /// decision, not an agent fault, so the task is cancelled rather than failed — which is what
    /// stops it consuming a retry and being reassigned moments later.
    pub fn kill_task(&self, task_id: TaskId) -> bool {
        let Some(handle) = self.sessions.get(&task_id) else {
            return false;
        };
        let killed = handle.kill_now().is_ok();
        drop(handle);
        // After the kill, so a dying agent's final report still has somewhere to land.
        self.report_servers.remove(&task_id);
        killed
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
        // Dropping the servers removes their sockets. Done after killing, so a dying agent's last
        // report still has somewhere to land.
        self.report_servers.clear();
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
                // Standing project knowledge. The task brief goes as the first message instead,
                // so it appears in the transcript the operator reads rather than being hidden in
                // the system prompt — but notes and skills are true of every task and would be
                // noise repeated in each brief.
                system_prompt: self.knowledge.as_deref(),
                model: self.model.as_deref(),
                extra_layers: vec![],
            })
            .await
            .map_err(|e| DispatchError::WorkspaceUnavailable {
                task_id: request.task_id,
                detail: e.to_string(),
            })?;

        let session_id = SessionId::new();

        // The reporting socket must exist before the agent starts: the CLI spawns its MCP servers
        // during startup, and a missing socket would leave the agent without the only tool that
        // can complete its task.
        let socket = socket_path(&self.runtime_dir, &session_id.to_string());
        let server = ReportServer::bind(socket.clone(), self.sink.clone())
            .await
            .map_err(|e| DispatchError::SpawnFailed {
                task_id: request.task_id,
                detail: format!("could not open the reporting socket: {e}"),
            })?;
        self.report_servers.insert(request.task_id, server);

        let mut opts = workspace.spawn_options(session_id);
        opts.config.mcp_config =
            Some(mcp_config(&self.mcp_binary, &socket, &request.task_id.to_string()).to_string());
        opts.attribution = Attribution {
            session_id: Some(session_id),
            agent_id: Some(request.agent_id),
            task_id: Some(request.task_id),
        };

        let argv: Vec<String> = opts
            .config
            .to_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let permission_mode = opts.config.permission_mode.as_flag().to_string();

        let (handle, join) = spawn_session(opts, self.bus.clone()).await.map_err(|e| {
            DispatchError::SpawnFailed {
                task_id: request.task_id,
                detail: e.to_string(),
            }
        })?;

        let handle = Arc::new(handle);

        // Recorded before the agent is given any work: the window between spawning a process and
        // knowing about it is exactly the window in which a crash produces an orphan nobody can
        // find. A failed write is logged rather than fatal — refusing to dispatch because
        // bookkeeping failed would be a worse outcome than an unrecorded process.
        let record = ProcessRecord {
            session_id: session_id.to_string(),
            task_id: Some(request.task_id.to_string()),
            pid: handle.pid(),
            pgid: handle.pid(),
            worktree_path: Some(workspace.worktree.path.clone()),
        };
        if let Err(e) = processes::record(&self.store, &self.boot, &record).await {
            tracing::error!(%e, "could not record a running agent process");
        }
        if let Err(e) = sessions::record_started(
            &self.store,
            &sessions::NewSession {
                session_id,
                agent_id: request.agent_id,
                project_id: self.identity.project_id.clone(),
                task_id: Some(request.task_id),
                // The worktree, which is what --resume must be re-invoked from. Recording
                // anything else would make the session unresumable.
                cwd: workspace.worktree.path.clone(),
                argv,
                model: self.model.clone(),
                permission_mode,
            },
        )
        .await
        {
            tracing::error!(%e, "could not record a session");
        }

        // Closes the records out when the agent exits, whatever the reason. Without this a clean
        // exit would leave a row claiming the process is still alive, and the next launch would
        // try to reap a pid that has since been reused by something unrelated.
        {
            let store = self.store.clone();
            tokio::spawn(async move {
                let reason = join
                    .await
                    .unwrap_or(deck_core::domain::event::ExitReason::Clean);
                let _ = sessions::record_ended(&store, session_id, &reason).await;
                let _ = processes::forget(&store, &session_id.to_string()).await;
            });
        }
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

    fn branch(&self, task_id: TaskId) -> Option<String> {
        self.registry
            .get(task_id)
            .map(|w| w.worktree.branch.clone())
    }

    fn agent_alive(&self, task_id: TaskId) -> Option<bool> {
        self.sessions.get(&task_id).map(|h| h.is_alive())
    }

    async fn integrate(
        &self,
        contributions: &[deck_core::git::Contribution],
        test_command: Option<&str>,
        timeout: std::time::Duration,
    ) -> deck_core::git::IntegrationOutcome {
        self.registry
            .worktrees()
            .integrate(
                self.registry.repo(),
                &self.base_ref,
                contributions,
                test_command,
                timeout,
            )
            .await
            // An integration that could not run is not a verdict on the work. Reporting it as a
            // failure would blame the agents for an environment problem.
            .unwrap_or_else(|e| deck_core::git::IntegrationOutcome::Inconclusive {
                reason: e.to_string(),
            })
    }
}

/// Locates the MCP server binary.
///
/// Beside the running executable, which is where it sits both in a cargo target directory and in a
/// packaged bundle. Falling back to a bare name would silently resolve to whatever is on PATH.
fn mcp_binary_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("deck-mcp")))
        .unwrap_or_else(|| PathBuf::from("deck-mcp"))
}

/// Routes worker reports into the supervisor.
///
/// The decision to accept a completion is not made here — this only records the claim and wakes
/// the loop, which then runs the verification gate. A sink that could accept a completion outright
/// would bypass the gate entirely, which is the one thing the whole design exists to prevent.
pub struct SupervisorSink {
    claims: SharedClaims,
    triggers: tokio::sync::mpsc::Sender<deck_supervisor::loop_engine::Trigger>,
}

impl SupervisorSink {
    pub fn new(
        claims: SharedClaims,
        triggers: tokio::sync::mpsc::Sender<deck_supervisor::loop_engine::Trigger>,
    ) -> Self {
        Self { claims, triggers }
    }
}

/// Reports waiting for the driver to pick up.
///
/// `parking_lot` rather than tokio: the driver drains this from a synchronous stage, and a lock it
/// could not acquire there would silently skip a completion claim.
pub type SharedClaims = Arc<parking_lot::Mutex<Vec<(TaskId, WorkerReport)>>>;

/// Hands queued reports to the driver at its IngestReports stage.
pub struct ClaimQueue(pub SharedClaims);

impl deck_supervisor::workspaces::ReportQueue for ClaimQueue {
    fn drain(&self) -> Vec<(TaskId, WorkerReport)> {
        std::mem::take(&mut *self.0.lock())
    }
}

/// Escalation answers the operator has given, waiting for the driver to apply them.
pub type SharedAnswers =
    Arc<parking_lot::Mutex<Vec<(String, deck_supervisor::escalation::EscalationAnswer)>>>;

/// Dispatch approvals the operator has clicked, waiting for the driver to act on them.
pub type SharedApprovals = Arc<parking_lot::Mutex<Vec<TaskId>>>;

pub struct GrantedApprovals(pub SharedApprovals);

/// Standing instructions the operator has sent, waiting for the driver.
pub type SharedGuidance = Arc<parking_lot::Mutex<Vec<deck_supervisor::guidance::Guidance>>>;

pub struct GivenGuidance(pub SharedGuidance);

impl deck_supervisor::guidance::GuidanceQueue for GivenGuidance {
    fn drain(&self) -> Vec<deck_supervisor::guidance::Guidance> {
        std::mem::take(&mut *self.0.lock())
    }
}

pub struct GivenAnswers(pub SharedAnswers);

impl deck_supervisor::escalation::AnswerQueue for GivenAnswers {
    fn drain(&self) -> Vec<(String, deck_supervisor::escalation::EscalationAnswer)> {
        std::mem::take(&mut *self.0.lock())
    }
}

impl deck_supervisor::autonomy::ApprovalQueue for GrantedApprovals {
    fn drain(&self) -> Vec<TaskId> {
        std::mem::take(&mut *self.0.lock())
    }
}

#[async_trait::async_trait]
impl ReportSink for SupervisorSink {
    async fn accept(&self, envelope: ReportEnvelope) -> ReportAck {
        let Ok(task_id) = envelope.task_id.parse::<uuid::Uuid>().map(TaskId::from) else {
            return ReportAck::rejected(format!(
                "report carried an unrecognised task id ({}); it was not recorded",
                envelope.task_id
            ));
        };

        let message = match &envelope.report {
            // Deliberately not "done". The claim is queued for verification, and saying otherwise
            // would let the agent believe it had finished before its criteria were checked.
            WorkerReport::ClaimTaskDone { .. } => {
                "Completion claimed. Your acceptance criteria will now be verified; if any fail the task comes back to you."
            }
            WorkerReport::ReportProgress { .. } => "Progress recorded.",
            WorkerReport::RaiseBlocker { .. } => {
                "Blocker recorded. The supervisor will decide how to proceed."
            }
        };

        self.claims.lock().push((task_id, envelope.report));
        // Wake the loop so the claim is acted on now rather than at the next tick.
        let _ = self
            .triggers
            .try_send(deck_supervisor::loop_engine::Trigger::ReportReceived);

        ReportAck::accepted(message)
    }
}
