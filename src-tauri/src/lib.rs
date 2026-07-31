mod events;
mod state;
mod supervision;

use state::AppState;

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
