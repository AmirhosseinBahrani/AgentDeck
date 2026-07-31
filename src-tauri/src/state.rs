use deck_core::bus::EventBus;
use deck_core::domain::event::EventEnvelope;
use deck_core::domain::ids::Seq;
use deck_core::permission::{worker_defaults, EffectivePolicy, PermissionBroker};
use deck_core::runtime::mock::{MockRuntime, Speed};
use deck_core::store::Store;
use deck_core::workspace::WorkspaceRegistry;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared application state.
///
/// The durable event receiver is drained into SQLite by a writer task that batches commits. If
/// that task stopped, the bounded channel would fill and backpressure the agents — which is the
/// intended failure mode: slowing an agent beats losing its audit log.
/// Captured `claude` v2.1.153 sessions, compiled in. Paths are relative to this source file.
const EMBEDDED_FIXTURES: &[(&str, &str)] = &[
    (
        "tool-use-session",
        include_str!("../../crates/deck-core/tests/fixtures/probe1.ndjson"),
    ),
    (
        "two-turn-session",
        include_str!("../../crates/deck-core/tests/fixtures/probe2.ndjson"),
    ),
    (
        "permission-denied-session",
        include_str!("../../crates/deck-core/tests/fixtures/probe4.ndjson"),
    ),
    (
        "permission-ask-session",
        include_str!("../../crates/deck-core/tests/fixtures/probe6-permission-ask.ndjson"),
    ),
];

pub struct AppState {
    pub bus: Arc<EventBus>,
    pub store: Store,
    pub mock: Arc<MockRuntime>,
    /// Sessions the UI is currently displaying. Deltas for anything else are dropped at the
    /// source rather than crossing the IPC bridge.
    pub watched: Arc<Mutex<Vec<deck_core::domain::ids::SessionId>>>,
    /// Per-agent workspaces: each real agent gets its own worktree and its own broker, so a
    /// boundary is one agent's directory rather than the union of everyone's.
    pub workspaces: Arc<WorkspaceRegistry>,
    /// Serves the fixture-replay demo only, which has no worktree of its own. Real agents never
    /// use it. It disappears once agents are launched from the UI in M4.
    pub demo_broker: Arc<PermissionBroker>,
    /// The agents of the current run, so the operator can stop them. `None` when no run is active.
    pub live_run: Arc<Mutex<Option<Arc<crate::supervision::LiveWorkspaces>>>>,
    /// Wakes the running loop. Also how cancellation reaches it.
    pub run_triggers:
        Arc<Mutex<Option<tokio::sync::mpsc::Sender<deck_supervisor::loop_engine::Trigger>>>>,
    /// Latest dashboard snapshot, refreshed by the loop after each iteration.
    pub run_snapshot: Arc<Mutex<Option<crate::events::RunSnapshot>>>,
    /// Worker reports awaiting the driver's IngestReports stage.
    pub pending_claims: crate::supervision::SharedClaims,
}

impl AppState {
    /// Fails only if the database cannot be opened or migrated, which is not recoverable: without
    /// it there is no audit log, and this app's guarantees rest on having one.
    pub async fn new() -> Result<Self, String> {
        let (bus, durable) = EventBus::new();

        let store = Store::open(Self::database_path())
            .await
            .map_err(|e| format!("could not open the AgentDeck database: {e}"))?;

        {
            let store = store.clone();
            // tauri::async_runtime, not tokio::spawn: AppState is constructed before the app
            // runs, so no tokio reactor exists yet and a bare tokio::spawn panics.
            tauri::async_runtime::spawn(async move {
                deck_core::store::events::run_writer(store, durable).await;
            });
        }

        let mock = Arc::new(MockRuntime::new(
            bus.clone(),
            Speed::FixedGap(std::time::Duration::from_millis(35)),
        ));

        // Embedded rather than read from disk so replay works in a packaged app, not only
        // from the source tree. These are real captured sessions, so the UI can be built and
        // demoed with no authenticated CLI and no rate-limit spend.
        for (name, body) in EMBEDDED_FIXTURES {
            mock.register(*name, *body);
        }

        // Until M3 gives each agent a worktree, containment is rooted at the current
        // directory. That makes the demo honest: the replayed out-of-tree write really is
        // outside this root, so it really does escalate.
        let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let policy =
            EffectivePolicy::resolve(root.canonicalize().unwrap_or(root), &[worker_defaults()]);
        let demo_broker = Arc::new(PermissionBroker::new(policy));
        mock.set_broker(demo_broker.clone());

        // Rooted at the process's working directory: real agent worktrees are created under
        // whichever project the operator opens, which arrives with the project model in M4.
        let workspaces = Arc::new(WorkspaceRegistry::new(
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        ));

        Ok(Self {
            bus,
            store,
            mock,
            watched: Arc::new(Mutex::new(Vec::new())),
            workspaces,
            demo_broker,
            live_run: Arc::new(Mutex::new(None)),
            run_triggers: Arc::new(Mutex::new(None)),
            run_snapshot: Arc::new(Mutex::new(None)),
            pending_claims: Arc::new(parking_lot::Mutex::new(Vec::new())),
        })
    }

    /// Beside the user's data directory rather than the current working directory, so a run does
    /// not leave a database inside whatever repository the app happened to be launched from.
    fn database_path() -> std::path::PathBuf {
        let base = dirs_next_home()
            .map(|h| h.join(".agentdeck"))
            .unwrap_or_else(|| std::path::PathBuf::from(".agentdeck"));
        let _ = std::fs::create_dir_all(&base);
        base.join("agentdeck.sqlite")
    }

    pub async fn events_since(&self, seq: Seq, limit: usize) -> Vec<EventEnvelope> {
        deck_core::store::events::since(&self.store, seq, limit)
            .await
            .unwrap_or_else(|e| {
                // A failed backfill leaves a gap in the UI rather than crashing it; the next batch
                // will report the gap again and it can retry.
                tracing::error!(%e, "event backfill failed");
                Vec::new()
            })
    }

    /// A session's recorded transcript, so reopening the app restores what the user was watching.
    pub async fn session_transcript(&self, session_id: &str, limit: usize) -> Vec<EventEnvelope> {
        deck_core::store::events::for_session(&self.store, session_id, limit)
            .await
            .unwrap_or_default()
    }
}

fn dirs_next_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
