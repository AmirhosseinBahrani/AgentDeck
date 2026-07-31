use deck_core::bus::EventBus;
use deck_core::domain::event::EventEnvelope;
use deck_core::domain::ids::Seq;
use deck_core::runtime::mock::{MockRuntime, Speed};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared application state.
///
/// The durable event receiver is drained into `history` for now. Once the store lands in the
/// app wiring this becomes the SQLite writer task; keeping an in-memory log until then means
/// `get_events_since` already works, so the frontend's gap-backfill path is exercised from
/// the start rather than being stubbed and forgotten.
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
];

pub struct AppState {
    pub bus: Arc<EventBus>,
    pub history: Arc<Mutex<Vec<EventEnvelope>>>,
    pub mock: Arc<MockRuntime>,
    /// Sessions the UI is currently displaying. Deltas for anything else are dropped at the
    /// source rather than crossing the IPC bridge.
    pub watched: Arc<Mutex<Vec<deck_core::domain::ids::SessionId>>>,
}

impl AppState {
    pub fn new() -> Self {
        let (bus, mut durable) = EventBus::new();
        let history: Arc<Mutex<Vec<EventEnvelope>>> = Arc::new(Mutex::new(Vec::new()));

        {
            let history = history.clone();
            // tauri::async_runtime, not tokio::spawn: AppState is constructed before the app
            // runs, so no tokio reactor exists yet and a bare tokio::spawn panics.
            tauri::async_runtime::spawn(async move {
                // Drains continuously. If this task stopped, the bounded durable channel
                // would fill and backpressure the agents rather than lose events.
                while let Some(envelope) = durable.recv().await {
                    history.lock().await.push(envelope);
                }
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

        Self {
            bus,
            history,
            mock,
            watched: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn events_since(&self, seq: Seq, limit: usize) -> Vec<EventEnvelope> {
        self.history
            .lock()
            .await
            .iter()
            .filter(|e| e.seq > seq)
            .take(limit)
            .cloned()
            .collect()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
