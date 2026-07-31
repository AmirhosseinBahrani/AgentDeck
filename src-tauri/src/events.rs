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
