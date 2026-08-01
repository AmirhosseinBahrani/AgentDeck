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

    // Before anything spawns. A .app opened from Finder gets a bare PATH with no Homebrew, nvm
    // or npm prefix on it, so `claude` — and the git and node it shells out to — would all be
    // unfindable. Repairing it here means every later consumer reads the corrected value.
    tauri::async_runtime::block_on(async {
        deck_core::runtime::shell_path::ensure_tool_on_path("claude").await
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
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
            events::list_session_history,
            events::get_project_memory,
            events::save_project_memory,
            events::list_skills,
            events::save_skill,
            events::delete_skill,
            events::create_project,
            events::suggest_project_location,
            events::import_project_knowledge,
            events::agent_metrics,
            events::get_permissions,
            events::save_permissions,
            events::get_models,
            events::save_models,
            events::resume_session,
            events::approve_dispatch,
            events::force_kill_agent,
            events::get_last_run,
            events::list_runs,
            events::clear_run,
            events::check_runtime,
            events::answer_escalation,
            events::send_guidance,
            events::get_task_diffs,
            events::get_task_patch,
            events::add_task,
            events::land_integration,
            events::pending_integration,
            events::list_project_files,
            events::read_project_file,
            events::get_project,
            events::set_project,
            events::list_projects,
            events::inspect_folder,
            events::init_project,
            events::list_agents,
            events::hire_agent,
            events::revoke_impact,
            events::revoke_agent,
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
