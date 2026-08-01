pub mod claude_code;
pub mod mock;
pub mod shell_path;

use crate::domain::event::EventEnvelope;
use crate::domain::ids::SessionId;
use claude_code::argv::{PermissionMode, SessionConfig};
use claude_code::wire::PermissionDecision;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("session {0} not found")]
    NoSuchSession(SessionId),
    #[error("session {0} is not accepting input")]
    NotWritable(SessionId),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

/// Abstraction over the underlying agent runtime, so orchestration never depends on the
/// CLI directly and can be driven by a fixture-replaying mock in tests.
#[async_trait::async_trait]
pub trait AgentRuntime: Send + Sync + 'static {
    async fn create_session(&self, cfg: SessionConfig) -> Result<SessionId>;

    /// Sessions bucket by cwd, so `cfg.cwd` must match the directory the session was
    /// originally created in or the CLI will not find the conversation.
    async fn resume_session(&self, cfg: SessionConfig) -> Result<SessionId>;

    async fn send_text(&self, id: SessionId, text: String) -> Result<()>;

    async fn respond_permission(
        &self,
        id: SessionId,
        request_id: String,
        decision: PermissionDecision,
    ) -> Result<()>;

    async fn set_permission_mode(&self, id: SessionId, mode: PermissionMode) -> Result<()>;

    /// Cooperative stop: lets the agent finish its turn and emit a final report.
    async fn interrupt(&self, id: SessionId) -> Result<()>;

    /// Unconditional, immediate termination of the whole process tree.
    ///
    /// Must not depend on the session actor being responsive — a wedged actor is exactly
    /// the case this exists for — and must not depend on the control channel or the NDJSON
    /// parser being healthy.
    async fn kill_now(&self, id: SessionId) -> Result<()>;

    fn subscribe(&self, id: SessionId) -> tokio::sync::broadcast::Receiver<EventEnvelope>;
}
