mod background;
mod events;
mod persistence;
mod state;
mod supervision;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,deck_core=debug".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .manage(
            // Blocking here is correct: without a database there is no audit log, and the app's
            // guarantees rest on having one. Starting up degraded would be worse than not starting.
            tauri::async_runtime::block_on(AppState::new())
                .expect("AgentDeck could not open its database"),
        )
        .invoke_handler(tauri::generate_handler![
            events::subscribe_events,
            events::set_session_subscriptions,
            events::get_events_since,
            events::get_session_transcript,
            events::replay_fixture,
            events::list_fixtures,
            events::respond_permission,
            events::pending_permission_count,
            events::start_supervisor_run,
            events::cancel_supervisor_run,
            events::get_run_snapshot,
            events::get_startup_recovery,
            events::get_resumable_sessions,
            events::resume_session,
            events::approve_dispatch,
            events::force_kill_agent,
            events::get_last_run,
        ])
        .setup(|app| {
            background::install_tray(app.handle())?;
            let bus = app.state::<AppState>().bus.clone();
            background::notify_on_attention(app.handle().clone(), bus);
            Ok(())
        })
        .on_window_event(background::intercept_close)
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
