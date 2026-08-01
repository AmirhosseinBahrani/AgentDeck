//! The event bridge to the webview.
//!
//! One IPC message per event does not survive several agents streaming at once, so events go
//! through the coalescer in `deck-core::ipc` and cross as batches on a `tauri::ipc::Channel`.
//! Anything the user must act on skips batching entirely.

use crate::state::AppState;
use deck_core::domain::event::EventEnvelope;
use deck_core::domain::ids::{Seq, SessionId};
use deck_core::domain::task::{TaskState, TaskStatus};
use deck_core::ipc::{is_critical, Coalescer, EventBatch, FLUSH_INTERVAL};
use tauri::ipc::Channel;
use tauri::State;

/// Starts streaming batched events to the frontend.
///
/// The frontend supplies a channel; Rust owns the cadence. Returns nothing — the first batch
/// arrives on the channel, and the frontend hydrates current state through separate queries.
#[tauri::command]
pub async fn subscribe_events(
    state: State<'_, AppState>,
    channel: Channel<EventBatch>,
) -> Result<(), String> {
    let mut observer = state.bus.subscribe();
    let watched = state.watched.clone();

    tokio::spawn(async move {
        let mut coalescer = Coalescer::new();
        let mut ticker = tokio::time::interval(FLUSH_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                received = observer.recv() => {
                    match received {
                        Ok(envelope) => {
                            // Sync the watch list each iteration; it changes on tab switches
                            // and only the visible session's deltas should cross the bridge.
                            coalescer.set_watched(watched.lock().await.clone());

                            if is_critical(&envelope.event) {
                                // Send immediately, but preserve ordering by flushing what is
                                // already buffered first — otherwise a permission request
                                // could arrive before the tool call that provoked it.
                                if let Some(batch) = coalescer.flush() {
                                    if channel.send(batch).is_err() {
                                        break;
                                    }
                                }
                                let solo = EventBatch {
                                    first_seq: Some(envelope.seq),
                                    last_seq: Some(envelope.seq),
                                    events: vec![envelope],
                                    deltas: Vec::new(),
                                };
                                if channel.send(solo).is_err() {
                                    break;
                                }
                                continue;
                            }

                            coalescer.push_event(envelope);
                            if coalescer.should_flush() {
                                if let Some(batch) = coalescer.flush() {
                                    if channel.send(batch).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                            // Deliberately not silent: the frontend detects the seq gap in the
                            // next batch and backfills. Log so the drop is diagnosable.
                            tracing::warn!(missed, "UI observer lagged; frontend will backfill");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }

                _ = ticker.tick() => {
                    if let Some(batch) = coalescer.flush() {
                        if channel.send(batch).is_err() {
                            break;
                        }
                    }
                }
            }
        }
    });

    Ok(())
}

/// Tells Rust which sessions the UI is showing, so deltas for the rest are dropped at the
/// source. This is the difference between a handful of agents and a dozen.
#[tauri::command]
pub async fn set_session_subscriptions(
    state: State<'_, AppState>,
    sessions: Vec<SessionId>,
) -> Result<(), String> {
    *state.watched.lock().await = sessions;
    Ok(())
}

/// Backfill after a detected gap.
///
/// `after` is a decimal string, not a number: seq is a `u64` and past 2^53 a JS number would
/// silently lose precision — and seq is exactly what gap detection depends on.
#[tauri::command]
pub async fn get_events_since(
    state: State<'_, AppState>,
    after: String,
    limit: Option<usize>,
) -> Result<Vec<EventEnvelope>, String> {
    let seq: u64 = after
        .parse()
        .map_err(|_| format!("invalid cursor {after:?}"))?;
    Ok(state.events_since(Seq(seq), limit.unwrap_or(1_000)).await)
}

/// Replays a captured session. Lets the whole UI be developed and demoed without an
/// authenticated CLI or spending any rate limit.
#[tauri::command]
pub async fn replay_fixture(state: State<'_, AppState>, name: String) -> Result<String, String> {
    let session = SessionId::new();
    let attribution = deck_core::bus::Attribution {
        session_id: Some(session),
        ..Default::default()
    };

    // Watch it immediately, otherwise the replay's deltas are correctly discarded and the
    // transcript looks empty — a confusing first-run experience.
    state.watched.lock().await.push(session);

    let mock = state.mock.clone();
    tokio::spawn(async move {
        mock.replay(&name, attribution).await;
    });

    Ok(session.to_string())
}

#[tauri::command]
pub async fn list_fixtures(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let mut names = state.mock.script_names();
    names.sort();
    Ok(names)
}

/// Applies an operator decision to a parked permission request.
///
/// Deliberately not optimistic on the frontend: a permission answer is a safety action, so the
/// UI must reflect what the backend actually accepted rather than assuming success.
#[tauri::command]
pub async fn respond_permission(
    state: State<'_, AppState>,
    request_id: String,
    allow: bool,
    updated_input: Option<serde_json::Value>,
) -> Result<(), String> {
    let resolution = if allow {
        deck_core::permission::Resolution::Allowed {
            updated_input: updated_input.unwrap_or(serde_json::Value::Null),
        }
    } else {
        deck_core::permission::Resolution::Denied {
            message: "The operator declined this action. Continue with an approach that stays \
                      inside your worktree, or report a blocker."
                .into(),
        }
    };

    // Try the real per-agent brokers first; fall back to the demo broker used by fixture
    // replay. Looking the request up rather than having the UI track which agent owns it keeps
    // safety-critical bookkeeping out of the frontend.
    match state
        .workspaces
        .resolve_permission(&request_id, resolution.clone())
    {
        Ok(()) => Ok(()),
        Err(_) => state
            .demo_broker
            .resolve(&request_id, resolution)
            .map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub async fn pending_permission_count(state: State<'_, AppState>) -> Result<usize, String> {
    Ok(state.workspaces.pending_permissions() + state.demo_broker.pending_count())
}

/// Starts a supervisor run against the current project.
///
/// Spawns the loop on a background task and returns immediately: a run can take many minutes, and
/// blocking the IPC call would freeze the UI for its duration. Progress reaches the frontend
/// through the event stream, which is already how everything else is observed.
#[tauri::command]
pub async fn start_supervisor_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    objective: String,
    max_cost_usd: Option<f64>,
    autonomy: Option<String>,
) -> Result<(), String> {
    use deck_supervisor::autonomy::Autonomy;
    use deck_supervisor::decision::PlanLimits;
    use deck_supervisor::driver::{Driver, Run, RunConfig, TeamMember};
    use deck_supervisor::loop_engine::RunLimits;
    use deck_supervisor::run_loop::{forward_event, RunLoop};

    // Refused rather than attempted. Without a repository there is nowhere to create worktrees
    // and nothing to verify against, and starting anyway would spend the rate limit producing
    // work with no home — the packaged app hits this whenever it is opened from Finder, which
    // gives it a working directory of `/`.
    let Some(repo) = state.project.clone() else {
        return Err(
            "AgentDeck is not inside a git repository, so there is nowhere for agents to work. \
             Launch it from a repository — `cd <your repo> && open -a AgentDeck .` — or run \
             `pnpm tauri dev` from one."
                .into(),
        );
    };

    // The three seeded roles. Team configuration arrives with the project model; hard-coding them
    // here keeps this honest about what exists rather than pretending to read config.
    //
    // Registered rather than freshly minted each run: an agent id that changed on every launch
    // would scatter one agent's history across a new row per run, and nothing downstream could
    // answer "what has the reviewer done".
    let mut team = Vec::new();
    for role in ["developer", "reviewer"] {
        let agent_id = deck_core::store::identity::ensure_agent(
            &state.store,
            &state.identity,
            deck_core::domain::ids::AgentId::new(),
            role,
        )
        .await
        .map_err(|e| format!("could not register the {role}: {e}"))?;
        team.push(TeamMember {
            agent_id,
            role: role.into(),
        });
    }

    let config = RunConfig {
        objective,
        team,
        default_test_command: "cargo test".into(),
        verification_root: repo.clone(),
        limits: RunLimits {
            max_cost_usd: max_cost_usd.unwrap_or(5.0),
            ..RunLimits::default()
        },
        plan_limits: PlanLimits::default(),
        per_call_budget_usd: 1.0,
        verification_timeout: std::time::Duration::from_secs(600),
        // Assisted by default. Autonomous has to be chosen, because the operator should not
        // discover that agents were running unattended by finding out what they did.
        autonomy: match autonomy.as_deref() {
            Some("manual") => Autonomy::Manual,
            Some("autonomous") => Autonomy::Autonomous,
            _ => Autonomy::Assisted,
        },
    };

    // Bounded: a full channel drops a redundant wake-up rather than backpressuring the event bus.
    // Losing one is safe because the tick picks the work up; stalling the bus would slow every
    // agent.
    let (triggers, trigger_rx) = tokio::sync::mpsc::channel(64);

    // Worker reports land here and are drained by the loop. A queue rather than direct mutation:
    // reports arrive from agent processes at arbitrary moments, and applying them mid-iteration
    // would mutate the graph underneath a stage that is reading it.
    let claims = state.pending_claims.clone();
    claims.lock().clear();

    let sink = std::sync::Arc::new(crate::supervision::SupervisorSink::new(
        claims.clone(),
        triggers.clone(),
    ));

    let workspaces = std::sync::Arc::new(crate::supervision::LiveWorkspaces::new(
        state.workspaces.clone(),
        state.bus.clone(),
        "main".into(),
        sink,
        state.store.clone(),
        state.boot.clone(),
        state.identity.clone(),
    ));
    let planner = crate::supervision::CliPlanner::new(repo);

    // Agent activity wakes the loop. Without this the run would only advance on the tick, which
    // would add up to a minute of latency to every handoff.
    {
        let mut observer = state.bus.subscribe();
        let triggers = triggers.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                match observer.recv().await {
                    Ok(envelope) => {
                        forward_event(&triggers, &envelope.event);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }

    *state.live_run.lock().await = Some(workspaces.clone());
    *state.run_triggers.lock().await = Some(triggers);

    let snapshot_slot = state.run_snapshot.clone();
    let objective_for_snapshot = config.objective.clone();
    let autonomy_for_snapshot = config.autonomy;

    let report_queue = crate::supervision::ClaimQueue(claims);
    let approvals = state.pending_approvals.clone();
    approvals.lock().clear();
    let approval_queue = crate::supervision::GrantedApprovals(approvals);
    let answers = state.pending_answers.clone();
    answers.lock().clear();
    let answer_queue = crate::supervision::GivenAnswers(answers);
    let app_handle = app.clone();

    // The supervisor has no long-lived model session on purpose: its memory is meant to *be* the
    // database. That only holds if the graph and the decision log reach disk, so they are
    // written after every iteration.
    let run_id = uuid::Uuid::new_v4().to_string();
    let meta_for_snapshot = RunMeta {
        // The same id the run is persisted under, shortened for display. Two ids for one run
        // would make the header and the database disagree about which run you are looking at.
        run_id: run_id.chars().take(4).collect(),
        started_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default(),
        team: config
            .team
            .iter()
            .map(|m| (m.agent_id, m.role.clone()))
            .collect(),
    };
    // Published before the loop starts. The first iteration cannot finish until the planner has
    // answered, which takes tens of seconds — and until this landed the dashboard showed the
    // "no run" screen the whole time, so pressing Start looked like it had done nothing.
    {
        let mut initial = snapshot_of(
            &config.objective,
            config.autonomy,
            &Run::new(),
            &meta_for_snapshot,
        );
        initial.active = true;
        initial.phase = "planning".into();
        *state.run_snapshot.lock() = Some(initial);
    }

    let persist = crate::persistence::RunWriter::new(
        state.store.clone(),
        run_id,
        state.identity.project_id.clone(),
        config.objective.clone(),
        config.autonomy.as_str().to_string(),
        config.limits.max_cost_usd,
    );

    tauri::async_runtime::spawn(async move {
        let driver = Driver::new(&config, &planner, workspaces.as_ref())
            .with_reports(&report_queue)
            .with_approvals(&approval_queue)
            .with_answers(&answer_queue);
        let mut run = Run::new();

        // Published after each iteration rather than polled from the run: polling would let the
        // dashboard render a picture from mid-iteration, where tasks and phase disagree.
        let observe = move |run: &Run| {
            let snapshot = snapshot_of(
                &objective_for_snapshot,
                autonomy_for_snapshot,
                run,
                &meta_for_snapshot,
            );
            // The tray is the only surface visible with the window closed, which is the normal
            // way a long run is watched.
            crate::background::update_tray(&app_handle, &snapshot);
            persist.record(run);
            // Written here rather than from a spawned task: iterations are ordered, and two
            // async writes racing is what made the dashboard flicker between states.
            *snapshot_slot.lock() = Some(snapshot);
        };

        let exit = RunLoop::new(trigger_rx)
            .observing(observe)
            .run(&driver, &mut run)
            .await;
        tracing::info!(
            ?exit,
            iterations = run.state.iteration,
            "supervisor run ended"
        );
        // A run that finishes while the operator is elsewhere is the case this exists for: they
        // started it precisely so they could stop watching.
        crate::background::notify_run_ended(&app, &exit, run.state.spent_usd);
    });

    Ok(())
}

/// Stops the current run and force-kills every agent it started.
///
/// Force-kill rather than a cooperative stop: a wedged agent may never answer, and the operator
/// asking to stop must always take effect immediately.
#[tauri::command]
pub async fn cancel_supervisor_run(state: State<'_, AppState>) -> Result<usize, String> {
    // Tell the loop first so it stops deciding, then kill the agents. The other order would let
    // one more iteration dispatch work moments after the operator asked to stop.
    if let Some(triggers) = state.run_triggers.lock().await.take() {
        let _ = triggers
            .send(deck_supervisor::loop_engine::Trigger::CancelRequested)
            .await;
    }

    let live = state.live_run.lock().await.take();
    match live {
        Some(workspaces) => Ok(workspaces.kill_all()),
        None => Ok(0),
    }
}

/// A session's recorded transcript.
///
/// The transcript store in the frontend is in-memory and per-run, so without this a restart shows
/// an empty pane for work that actually happened. Reading from the event log is what makes the
/// durable log worth keeping.
#[tauri::command]
pub async fn get_session_transcript(
    state: State<'_, AppState>,
    session_id: String,
    limit: Option<usize>,
) -> Result<Vec<EventEnvelope>, String> {
    Ok(state
        .session_transcript(&session_id, limit.unwrap_or(2_000))
        .await)
}

/// Force-kills one agent, leaving the rest of the run alone.
///
/// Unconditional and immediate. The agent most in need of killing is the one that has stopped
/// answering, so this never waits on the session, the control channel, or the supervisor loop.
///
/// Nothing is lost: the worktree and its branch survive with their changes, and the task is
/// cancelled rather than failed, so it does not consume a retry or get reassigned on its own.
#[tauri::command]
pub async fn force_kill_agent(state: State<'_, AppState>, task_id: String) -> Result<bool, String> {
    let id = task_id
        .parse::<uuid::Uuid>()
        .map(deck_core::domain::ids::TaskId::from)
        .map_err(|_| format!("{task_id} is not a task id"))?;

    let live = state.live_run.lock().await.clone();
    match live {
        Some(workspaces) => Ok(workspaces.kill_task(id)),
        None => Ok(false),
    }
}

/// Whether the `claude` CLI is installed and someone has logged into it.
///
/// AgentDeck has no accounts of its own, so this is not a sign-in — it is the one dependency the
/// app cannot satisfy for the operator. Checked at startup rather than at first run, because
/// otherwise a missing CLI surfaces as an agent that failed to spawn several minutes after they
/// wrote an objective, and reads as a problem with the objective.
#[tauri::command]
pub async fn check_runtime() -> Result<deck_core::runtime::claude_code::probe::Readiness, String> {
    Ok(deck_core::runtime::claude_code::probe::probe("claude").await)
}

/// What the last run in this project was doing.
///
/// The point of persisting the graph and the decision log is that reopening the app is not a
/// blank screen: the operator can see what was asked for, how far it got, and what the
/// supervisor decided — including for a run that ended while they were away.
#[derive(serde::Serialize)]
pub struct PastRunSummary {
    pub run_id: String,
    pub objective: String,
    pub status: String,
    pub autonomy: String,
    pub iteration: u32,
    pub spent_usd: f64,
    pub task_count: i64,
    pub decision_count: i64,
}

#[tauri::command]
pub async fn get_last_run(state: State<'_, AppState>) -> Result<Option<PastRunSummary>, String> {
    Ok(state.last_run().await.map(|run| PastRunSummary {
        run_id: run.run_id,
        objective: run.objective,
        status: run.status,
        autonomy: run.autonomy,
        iteration: run.iteration,
        spent_usd: run.spent_usd,
        task_count: run.task_count,
        decision_count: run.decision_count,
    }))
}

/// Applies a typed answer to an open escalation.
///
/// The answer is an enum rather than free text on purpose. The whole design rests on the model
/// never widening its own permissions, and a prose channel into the supervisor would be exactly
/// that hole — "just skip the tests" has to be unrepresentable, not merely discouraged.
#[tauri::command]
pub async fn answer_escalation(
    state: State<'_, AppState>,
    escalation_id: String,
    answer: deck_supervisor::escalation::EscalationAnswer,
) -> Result<(), String> {
    state.pending_answers.lock().push((escalation_id, answer));

    // Wake the loop so the run resumes now. It is parked rather than finished, and without this
    // it would sit until the next tick wondering why nothing had changed.
    if let Some(triggers) = state.run_triggers.lock().await.as_ref() {
        let _ = triggers.try_send(deck_supervisor::loop_engine::Trigger::HumanAnswered);
    }
    Ok(())
}

/// Lets a task start.
///
/// One approval starts one agent. Deliberately not a standing grant for the task: a retry after
/// a failure is a new agent doing new work, and in a mode where the operator asked to approve
/// each start, silently reusing an old approval would not be approval.
#[tauri::command]
pub async fn approve_dispatch(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let id = task_id
        .parse::<uuid::Uuid>()
        .map(deck_core::domain::ids::TaskId::from)
        .map_err(|_| format!("{task_id} is not a task id"))?;

    state.pending_approvals.lock().push(id);

    // Wake the loop so the agent starts now rather than at the next tick. Without this an
    // approval appears to do nothing for a second or two, which reads as a broken button.
    if let Some(triggers) = state.run_triggers.lock().await.as_ref() {
        let _ = triggers.try_send(deck_supervisor::loop_engine::Trigger::HumanAnswered);
    }
    Ok(())
}

/// What starting up had to clean up after a previous launch.
///
/// Surfaced rather than logged: an operator who force-quit the app mid-run needs to know their
/// agents were killed and which sessions survived, or they will assume work is still in flight.
#[tauri::command]
pub async fn get_startup_recovery(
    state: State<'_, AppState>,
) -> Result<crate::state::RecoveryReport, String> {
    Ok(state.startup_recovery.clone())
}

/// Sessions a crash interrupted, each with the directory they must be resumed from.
#[derive(serde::Serialize)]
pub struct ResumableSummary {
    pub session_id: String,
    pub task_id: Option<String>,
    pub cwd: String,
    pub status: String,
    /// False once the worktree is gone, which makes the conversation permanently unreachable.
    /// Shown up front rather than discovered when a resume fails.
    pub resumable: bool,
}

#[tauri::command]
pub async fn get_resumable_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<ResumableSummary>, String> {
    Ok(state
        .resumable_sessions()
        .await
        .into_iter()
        .map(|s| ResumableSummary {
            session_id: s.session_id.to_string(),
            task_id: s.task_id.map(|t| t.to_string()),
            cwd: s.cwd.display().to_string(),
            status: s.status,
            resumable: s.cwd_exists,
        })
        .collect())
}

/// Reopens an interrupted session in the directory it originally ran in.
///
/// The directory is not a convenience here: Claude buckets conversations by working directory,
/// so resuming from anywhere else fails outright with "no conversation found". That is why the
/// recorded cwd is used verbatim and a missing one is refused rather than substituted.
#[tauri::command]
pub async fn resume_session(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<String, String> {
    use deck_core::permission::{worker_defaults, EffectivePolicy, PermissionBroker};
    use deck_core::runtime::claude_code::actor::{spawn_session, SpawnOptions};
    use deck_core::runtime::claude_code::argv::{PermissionMode, SessionConfig};

    let target = session_id
        .parse::<uuid::Uuid>()
        .map(SessionId::from)
        .map_err(|_| format!("{session_id} is not a session id"))?;

    let session = state
        .resumable_sessions()
        .await
        .into_iter()
        .find(|s| s.session_id == target)
        .ok_or_else(|| "that session is not resumable".to_string())?;

    if !session.cwd_exists {
        return Err(format!(
            "the worktree this session ran in is gone ({}), so its conversation cannot be \
             reopened",
            session.cwd.display()
        ));
    }

    let mut config = SessionConfig::new(SessionId::new(), session.cwd.clone());
    // --resume replaces --session-id rather than accompanying it, so the id above is discarded
    // by argv building; the resumed conversation keeps its original identity.
    config.resume = Some(target);
    config.permission_mode = PermissionMode::AcceptEdits;
    config.tools = vec![
        "Read".into(),
        "Write".into(),
        "Edit".into(),
        "Glob".into(),
        "Grep".into(),
        "Bash".into(),
    ];

    // Containment is re-derived from the worktree rather than restored from the old session:
    // policy must reflect what is allowed now, not what was allowed when the session started.
    let policy = EffectivePolicy::resolve(
        session.cwd.canonicalize().unwrap_or(session.cwd.clone()),
        &[worker_defaults()],
    );

    let mut opts = SpawnOptions::new(config);
    opts.broker = Some(std::sync::Arc::new(PermissionBroker::new(policy)));
    opts.attribution = deck_core::bus::Attribution {
        session_id: Some(target),
        agent_id: None,
        task_id: session.task_id,
    };

    let (handle, _join) = spawn_session(opts, state.bus.clone())
        .await
        .map_err(|e| format!("could not reopen the session: {e}"))?;

    Ok(handle.session_id.to_string())
}

/// A snapshot of the current run for the dashboard.
///
/// A single query rather than several: the Team View needs objective, phase, agents and task
/// counts to agree with each other, and fetching them separately would let the UI render a
/// half-updated picture mid-iteration.
#[derive(serde::Serialize, Default, Clone)]
pub struct RunSnapshot {
    pub active: bool,
    pub objective: String,
    pub phase: String,
    pub iteration: u32,
    pub spent_usd: f64,
    pub open_escalations: usize,
    /// "manual" | "assisted" | "autonomous". Drives what the UI offers, and the accent stripe
    /// that tells the operator at a glance what agents may do without asking.
    pub autonomy: String,
    /// Whether the branches have been merged and tested together. A run is not finished without
    /// it, so the dashboard says so rather than showing all-green tasks and nothing else.
    pub integrated: bool,
    /// The questions the run needs answered, with the answers it will accept.
    pub escalations: Vec<deck_supervisor::escalation::Escalation>,
    pub tasks: Vec<TaskSummary>,
    pub decisions: Vec<DecisionSummary>,
    /// Short run identifier, for the header. The full uuid is unreadable at a glance and the
    /// operator only ever needs enough of it to tell two runs apart.
    pub run_id: String,
    pub started_at_ms: i64,
    /// The team, as the roster shows it: who exists, what they are doing right now, and where.
    pub agents: Vec<AgentSummary>,
    /// Dependency edges, so the task graph can be drawn as the graph it already is rather than
    /// flattened into a list.
    pub edges: Vec<GraphEdge>,
    /// How many agents may run at once, and how many are.
    pub max_concurrent: usize,
    pub engaged: usize,
}

/// One member of the team.
#[derive(serde::Serialize, Clone)]
pub struct AgentSummary {
    pub id: String,
    /// "Developer", "Reviewer" — derived from the role, since agents have no separate name yet.
    pub name: String,
    pub role: String,
    /// running | blocked | idle — what the roster colours and sorts on.
    pub status: String,
    /// The task they are on, phrased as the activity it is.
    pub activity: Option<String>,
    pub task_id: Option<String>,
    pub session_id: Option<String>,
    /// The worktree branch their work lives on.
    pub branch: Option<String>,
    /// Attempts and review rounds for the current task, which is the only honest progress signal
    /// available — nothing reports a percentage, and inventing one would misreport how far along
    /// real work is.
    pub attempts: u32,
    pub review_rounds: u32,
}

#[derive(serde::Serialize, Clone)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    /// "hard" blocks readiness; "soft" only orders the work.
    pub kind: String,
}

#[derive(serde::Serialize, Clone)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub role: String,
    pub attempts: u32,
    pub review_rounds: u32,
    pub objective_gate: bool,
    pub blocked_reason: Option<String>,
    /// Assigned and ready, but held because this mode requires a human to start it.
    pub awaiting_approval: bool,
    /// The agent's session, so its transcript is reachable from the task.
    ///
    /// Without this a real agent was unfindable: the sessions list was only ever populated by
    /// fixture replay, so the one thing an operator wants when a task looks stuck — to read what
    /// the agent is actually doing — had no route to it.
    pub session_id: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub struct DecisionSummary {
    pub iteration: u32,
    pub stage: String,
    pub kind: String,
    /// "code", "claude" or "human" — the dashboard's most useful column, because it answers
    /// whether the model was actually driving or code kept falling back.
    pub decided_by: String,
    pub rationale: String,
    pub repaired: bool,
}

#[tauri::command]
pub async fn get_run_snapshot(state: State<'_, AppState>) -> Result<RunSnapshot, String> {
    let snapshot = state.run_snapshot.lock().clone();
    Ok(snapshot.unwrap_or_default())
}

/// Identity and team of the current run, which the graph itself does not carry.
#[derive(Clone)]
pub struct RunMeta {
    pub run_id: String,
    pub started_at_ms: i64,
    pub team: Vec<(deck_core::domain::ids::AgentId, String)>,
}

/// A role rendered as the job it is. Agents have no separate name yet, and "developer" in a
/// roster of people reads as a placeholder where "Developer" reads as a seat on the team.
fn display_name(role: &str) -> String {
    match role {
        "developer" => "Developer".into(),
        "reviewer" => "Reviewer".into(),
        "supervisor" => "Supervisor".into(),
        other => {
            let mut c = other.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Flattens a run into what the dashboard shows.
fn snapshot_of(
    objective: &str,
    autonomy: deck_supervisor::autonomy::Autonomy,
    run: &deck_supervisor::driver::Run,
    meta: &RunMeta,
) -> RunSnapshot {
    let tasks = run
        .graph
        .tasks()
        .map(|task| TaskSummary {
            id: task.id.to_string(),
            title: run
                .titles
                .get(&task.id)
                .cloned()
                .unwrap_or_else(|| task.id.to_string()),
            status: format!("{:?}", task.status).to_lowercase(),
            role: run.roles.get(&task.id).cloned().unwrap_or_default(),
            attempts: task.attempts,
            review_rounds: task.review_rounds,
            objective_gate: task.objective_gate,
            blocked_reason: task.failure_reason.clone(),
            awaiting_approval: run.awaiting_approval.contains(&task.id),
            session_id: run.sessions.get(&task.id).map(|s| s.to_string()),
        })
        .collect();

    // Newest first, and capped: the decision log grows for the life of a run, and the panel only
    // ever shows the recent tail.
    let decisions = run
        .log
        .decisions
        .iter()
        .rev()
        .take(50)
        .map(|d| DecisionSummary {
            iteration: d.iteration,
            stage: d.stage.clone(),
            kind: d.kind.clone(),
            decided_by: format!("{:?}", d.decided_by).to_lowercase(),
            rationale: d.rationale.clone(),
            repaired: d.repair_count > 0,
        })
        .collect();

    // One row per team member rather than per task: the roster answers "who is working", and an
    // agent between tasks still exists and still occupies a slot.
    let agents: Vec<AgentSummary> = meta
        .team
        .iter()
        .map(|(agent_id, role)| {
            // Sorted before picking. The graph stores tasks in a HashMap, so iteration order
            // differs between calls — an agent holding two active tasks would appear to be on a
            // different one each time the dashboard polled, and its status would flicker.
            let mut mine: Vec<&TaskState> = run
                .graph
                .tasks()
                .filter(|t| t.assignee == Some(*agent_id))
                .collect();
            mine.sort_by_key(|t| t.id);

            let current = mine
                .iter()
                .find(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Review))
                .or_else(|| mine.iter().find(|t| t.status == TaskStatus::Blocked))
                .copied();

            let status = match current.map(|t| t.status) {
                Some(TaskStatus::Running) | Some(TaskStatus::Review) => "running",
                Some(TaskStatus::Blocked) => "blocked",
                _ => "idle",
            };

            AgentSummary {
                id: agent_id.to_string(),
                name: display_name(role),
                role: role.clone(),
                status: status.to_string(),
                activity: current.and_then(|t| run.titles.get(&t.id).cloned()),
                task_id: current.map(|t| t.id.to_string()),
                session_id: current
                    .and_then(|t| run.sessions.get(&t.id))
                    .map(|s| s.to_string()),
                branch: current.and_then(|t| run.branches.get(&t.id).cloned()),
                attempts: current.map(|t| t.attempts).unwrap_or(0),
                review_rounds: current.map(|t| t.review_rounds).unwrap_or(0),
            }
        })
        .collect();

    let engaged = agents.iter().filter(|a| a.status == "running").count();

    let edges = run
        .graph
        .edges()
        .iter()
        .map(|e| GraphEdge {
            from: e.from.to_string(),
            to: e.to.to_string(),
            kind: format!("{:?}", e.kind).to_lowercase(),
        })
        .collect();

    RunSnapshot {
        run_id: meta.run_id.clone(),
        started_at_ms: meta.started_at_ms,
        max_concurrent: meta.team.len(),
        engaged,
        agents,
        edges,
        active: !matches!(
            run.state.phase,
            deck_supervisor::loop_engine::RunPhase::Completed
                | deck_supervisor::loop_engine::RunPhase::Failed
                | deck_supervisor::loop_engine::RunPhase::Cancelled
        ),
        objective: objective.to_string(),
        autonomy: autonomy.as_str().to_string(),
        integrated: run.state.integrated,
        escalations: run.escalations.clone(),
        phase: format!("{:?}", run.state.phase).to_lowercase(),
        iteration: run.state.iteration,
        spent_usd: run.state.spent_usd,
        open_escalations: run.state.open_escalations,
        tasks,
        decisions,
    }
}
