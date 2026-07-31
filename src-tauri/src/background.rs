//! Running unattended.
//!
//! A supervisor run takes tens of minutes and the operator is not expected to watch it. That
//! makes two things load-bearing rather than cosmetic. Closing the window must not kill the
//! team — agents are real processes holding real worktrees, and losing them to a reflexive ⌘W
//! destroys work that cannot be recovered. And when the run needs a human, it has to be able to
//! reach one who is looking at something else entirely, which is what the tray and the OS
//! notification are for.

use crate::state::AppState;
use deck_core::bus::EventBus;
use deck_core::domain::event::AgentEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime, WindowEvent};
use tauri_plugin_notification::NotificationExt;

/// Tray menu item ids. Matched by string because that is the API's own contract.
const ID_SHOW: &str = "show";
const ID_STOP: &str = "stop";
const ID_QUIT: &str = "quit";

/// True once the user has chosen Quit, so the close handler stops intercepting.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Builds the tray icon and its menu.
pub fn install_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, ID_SHOW, "Open AgentDeck", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, ID_STOP, "Stop the run", true, None::<&str>)?;
    // Distinguished from closing the window on purpose: the whole point of the tray is that
    // closing does not stop anything, so ending the team has to be its own deliberate action.
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit and stop all agents", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &stop, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("AgentDeck — idle")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            ID_SHOW => reveal(app),
            ID_STOP => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { stop_run(&app).await });
            }
            ID_QUIT => {
                QUITTING.store(true, Ordering::SeqCst);
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    // Agents are killed before exiting rather than left to the OS: on Unix a
                    // process group outlives its parent, so quitting without this is exactly
                    // how the orphans that startup has to reap get created.
                    stop_run(&app).await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick { .. } = event {
                reveal(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Reflects the run in the tray tooltip.
///
/// The tooltip is the only thing an operator can see without reopening the window, so it says
/// whether anyone is waiting on them rather than just that something is running.
pub fn update_tray<R: Runtime>(app: &AppHandle<R>, snapshot: &crate::events::RunSnapshot) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let text = if !snapshot.active {
        "AgentDeck — idle".to_string()
    } else if snapshot.open_escalations > 0 {
        format!(
            "AgentDeck — waiting on you ({} to answer)",
            snapshot.open_escalations
        )
    } else {
        let running = snapshot
            .tasks
            .iter()
            .filter(|t| t.status == "running")
            .count();
        format!(
            "AgentDeck — {running} running, iteration {}",
            snapshot.iteration
        )
    };
    let _ = tray.set_tooltip(Some(text));
}

/// Keeps a run alive when the window is closed.
///
/// Returns without preventing the close when nothing is running, so the app behaves like an
/// ordinary window whenever hiding would just be confusing.
pub fn intercept_close<R: Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if QUITTING.load(Ordering::SeqCst) {
        return;
    }

    let state = window.app_handle().state::<AppState>();
    let run_is_live = state
        .live_run
        .try_lock()
        .map(|guard| guard.is_some())
        .unwrap_or(true);

    if !run_is_live {
        return;
    }

    api.prevent_close();
    let _ = window.hide();
    let _ = window.app_handle().notification().builder()
        .title("AgentDeck is still running")
        .body("Your agents are still working. Open AgentDeck from the tray, or quit from there to stop them.")
        .show();
}

/// Sends an OS notification when a run needs the operator.
///
/// Only fires while the window is not focused. A notification for something already on screen
/// is noise, and noise is what makes people turn notifications off — at which point the one
/// that matters never arrives.
pub fn notify_on_attention<R: Runtime>(app: AppHandle<R>, bus: Arc<EventBus>) {
    tauri::async_runtime::spawn(async move {
        let mut observer = bus.subscribe();
        let mut last_sent: Option<Instant> = None;

        loop {
            let envelope = match observer.recv().await {
                Ok(envelope) => envelope,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };

            let Some((title, body)) = attention_message(&envelope.event) else {
                continue;
            };

            if app
                .get_webview_window("main")
                .and_then(|w| w.is_focused().ok())
                .unwrap_or(false)
            {
                continue;
            }

            // One notification per burst. Several agents hitting the same boundary at once is
            // common, and a stack of near-identical banners is worse than a single one.
            if last_sent.is_some_and(|at| at.elapsed() < NOTIFY_COOLDOWN) {
                continue;
            }
            last_sent = Some(Instant::now());

            let _ = app.notification().builder().title(title).body(body).show();
        }
    });
}

const NOTIFY_COOLDOWN: Duration = Duration::from_secs(30);

/// Tells the operator how a run ended, whatever they are doing.
///
/// Not rate-limited and not suppressed when the window is focused: a run ends once, and it is
/// the single moment the operator started the run in order to be told about.
pub fn notify_run_ended<R: Runtime>(
    app: &AppHandle<R>,
    exit: &deck_supervisor::run_loop::LoopExit,
    spent_usd: f64,
) {
    use deck_supervisor::run_loop::LoopExit;

    let (title, body) = match exit {
        LoopExit::Terminal(phase) => (
            format!("Run {}", format!("{phase:?}").to_lowercase()),
            format!("Spent ${spent_usd:.2}."),
        ),
        LoopExit::Cancelled => (
            "Run stopped".to_string(),
            format!("You stopped the run. Spent ${spent_usd:.2}."),
        ),
        // Nothing can wake the loop again, which is a defect rather than an outcome — worth
        // saying plainly instead of dressing it up as a normal ending.
        LoopExit::ChannelClosed => (
            "Run ended unexpectedly".to_string(),
            "The supervisor lost its event stream and could not continue.".to_string(),
        ),
    };

    let _ = app.notification().builder().title(title).body(body).show();
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some("AgentDeck — idle"));
    }
}

/// Which events are worth interrupting someone for.
///
/// Deliberately short. Progress is not an interruption; only a decision nobody but the operator
/// can make, or work stopping unexpectedly, earns a banner.
fn attention_message(event: &AgentEvent) -> Option<(&'static str, String)> {
    match event {
        AgentEvent::PermissionRequest { tool, .. } => Some((
            "An agent needs your approval",
            format!("{tool} was blocked and is waiting for you to decide."),
        )),
        AgentEvent::SessionExited {
            reason: deck_core::domain::event::ExitReason::StartupFailed { detail },
        } => Some(("An agent could not start", detail.clone())),
        _ => None,
    }
}

fn reveal<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

async fn stop_run<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if let Some(triggers) = state.run_triggers.lock().await.take() {
        let _ = triggers
            .send(deck_supervisor::loop_engine::Trigger::CancelRequested)
            .await;
    }
    let live = state.live_run.lock().await.take();
    if let Some(workspaces) = live {
        workspaces.kill_all();
    }
}
