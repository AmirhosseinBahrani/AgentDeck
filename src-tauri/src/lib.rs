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
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            events::subscribe_events,
            events::set_session_subscriptions,
            events::get_events_since,
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
