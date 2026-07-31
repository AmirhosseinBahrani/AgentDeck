//! The event bridge to the webview.
//!
//! One IPC message per event does not survive several agents streaming at once, so events go
//! through the coalescer in `deck-core::ipc` and cross as batches on a `tauri::ipc::Channel`.
//! Anything the user must act on skips batching entirely.

use crate::state::AppState;
use deck_core::domain::event::EventEnvelope;
use deck_core::domain::ids::{Seq, SessionId};
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
    state: State<'_, AppState>,
    objective: String,
    max_cost_usd: Option<f64>,
) -> Result<(), String> {
    use deck_supervisor::decision::PlanLimits;
    use deck_supervisor::driver::{Driver, Run, RunConfig, TeamMember};
    use deck_supervisor::loop_engine::RunLimits;
    use deck_supervisor::run_loop::{forward_event, RunLoop};

    let repo = state.workspaces.repo().to_path_buf();

    // The three seeded roles. Team configuration arrives with the project model; hard-coding them
    // here keeps this honest about what exists rather than pretending to read config.
    let team = vec![
        TeamMember {
            agent_id: deck_core::domain::ids::AgentId::new(),
            role: "developer".into(),
        },
        TeamMember {
            agent_id: deck_core::domain::ids::AgentId::new(),
            role: "reviewer".into(),
        },
    ];

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
    };

    let workspaces = std::sync::Arc::new(crate::supervision::LiveWorkspaces::new(
        state.workspaces.clone(),
        state.bus.clone(),
        "main".into(),
    ));
    let planner = crate::supervision::CliPlanner::new(repo);

    // Bounded: a full channel drops a redundant wake-up rather than backpressuring the event bus.
    // Losing one is safe because the tick picks the work up; stalling the bus would slow every
    // agent.
    let (triggers, trigger_rx) = tokio::sync::mpsc::channel(64);

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

    tauri::async_runtime::spawn(async move {
        let driver = Driver::new(&config, &planner, workspaces.as_ref());
        let mut run = Run::new();

        let exit = RunLoop::new(trigger_rx).run(&driver, &mut run).await;
        tracing::info!(
            ?exit,
            iterations = run.state.iteration,
            "supervisor run ended"
        );
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
