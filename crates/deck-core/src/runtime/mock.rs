//! Fixture-replaying runtime.
//!
//! Spawning real `claude` processes costs tokens against a subscription rate limit, so it is
//! not viable as the default way to exercise the app. This replays captured NDJSON through
//! the same translator the live runtime uses, at a configurable rate, which makes three
//! things possible: developing the frontend without an account, load-testing event
//! throughput far above real speed, and — most importantly — testing the supervisor state
//! machine deterministically, which is otherwise impossible against a live model.

use crate::bus::{Attribution, EventBus};
use crate::domain::event::{AgentEvent, ExitReason};
use crate::domain::ids::SessionId;
use crate::runtime::claude_code::translate::Translator;
use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How fast to replay relative to the captured timing.
#[derive(Debug, Clone, Copy)]
pub enum Speed {
    /// No delay between lines. Used for load tests and fast unit tests.
    Immediate,
    /// A fixed gap between lines, to approximate a live stream for UI work.
    FixedGap(Duration),
}

pub struct MockSession {
    pub session_id: SessionId,
    /// Lines actually emitted so far. Lets a load test assert full delivery.
    pub emitted: Arc<AtomicUsize>,
}

/// Replays fixtures as sessions.
pub struct MockRuntime {
    bus: Arc<EventBus>,
    speed: Speed,
    /// Scripted responses keyed by fixture name, so a test can register its own NDJSON
    /// rather than relying on files on disk.
    scripts: DashMap<String, Vec<String>>,
}

impl MockRuntime {
    pub fn new(bus: Arc<EventBus>, speed: Speed) -> Self {
        Self {
            bus,
            speed,
            scripts: DashMap::new(),
        }
    }

    /// Registers an inline script. Useful for supervisor tests that need one specific
    /// decision shape rather than a whole recorded session.
    pub fn register(&self, name: impl Into<String>, ndjson: impl Into<String>) {
        let lines = ndjson
            .into()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        self.scripts.insert(name.into(), lines);
    }

    pub fn register_fixture_dir(&self, dir: &std::path::Path) -> std::io::Result<usize> {
        let mut count = 0;
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ndjson") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            self.register(name, std::fs::read_to_string(&path)?);
            count += 1;
        }
        Ok(count)
    }

    /// Replays a registered script as one session, publishing to the bus exactly as the live
    /// runtime does. Returns once the script is exhausted.
    pub async fn replay(
        &self,
        name: &str,
        attribution: Attribution,
    ) -> Option<(MockSession, ExitReason)> {
        let lines = self.scripts.get(name)?.clone();
        let session_id = attribution.session_id.unwrap_or_default();
        let emitted = Arc::new(AtomicUsize::new(0));

        let mut translator = Translator::new();
        let mut attribution = attribution;
        attribution.session_id = Some(session_id);

        for line in &lines {
            if let Speed::FixedGap(gap) = self.speed {
                tokio::time::sleep(gap).await;
            }
            for event in translator.translate(line) {
                self.bus.publish(attribution, event).await;
            }
            emitted.fetch_add(1, Ordering::Relaxed);
        }

        self.bus
            .publish(
                attribution,
                AgentEvent::SessionExited {
                    reason: ExitReason::Clean,
                },
            )
            .await;

        Some((
            MockSession {
                session_id,
                emitted,
            },
            ExitReason::Clean,
        ))
    }

    /// Replays the same script as `n` concurrent sessions. This is the load-test entry
    /// point: it reproduces the shape that actually stresses the app — several agents
    /// streaming at once — without any of them being real.
    pub async fn replay_concurrent(
        self: &Arc<Self>,
        name: &str,
        sessions: usize,
    ) -> Vec<SessionId> {
        let mut handles = Vec::with_capacity(sessions);
        for _ in 0..sessions {
            let me = self.clone();
            let script = name.to_string();
            handles.push(tokio::spawn(async move {
                let attribution = Attribution {
                    session_id: Some(SessionId::new()),
                    ..Default::default()
                };
                me.replay(&script, attribution)
                    .await
                    .map(|(s, _)| s.session_id)
            }));
        }

        let mut ids = Vec::new();
        for h in handles {
            if let Ok(Some(id)) = h.await {
                ids.push(id);
            }
        }
        ids
    }

    pub fn script_names(&self) -> Vec<String> {
        self.scripts.iter().map(|e| e.key().clone()).collect()
    }
}
