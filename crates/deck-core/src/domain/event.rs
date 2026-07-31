use super::ids::{AgentId, Seq, SessionId, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// AgentDeck's own event model. Deliberately independent of the CLI's output format so
/// the rest of the app is insulated from upstream changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    /// First `system/init` of a process means startup succeeded. Note the CLI re-emits
    /// init at every turn, so only the first should be treated as a handshake.
    SessionReady {
        cwd: String,
        model: Option<String>,
        tools: Vec<String>,
    },
    TurnStarted,
    Message {
        role: String,
        content: Value,
    },
    ToolCall {
        tool_use_id: String,
        tool: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        output: Value,
        is_error: bool,
    },
    /// A `can_use_tool` control request awaiting a decision.
    PermissionRequest {
        request_id: String,
        tool: String,
        input: Value,
        /// CLI-supplied classification, e.g. `workingDir`.
        reason_type: Option<String>,
        blocked_path: Option<String>,
        suggestions: Vec<Value>,
    },
    PermissionResolved {
        request_id: String,
        allowed: bool,
    },
    /// Structured, proactive throttling signal carrying a reset timestamp.
    RateLimited {
        status: String,
        resets_at: Option<i64>,
        limit_type: Option<String>,
    },
    /// End of a turn. Cost is per-turn and must be summed, not replaced.
    TurnComplete {
        subtype: String,
        is_error: bool,
        cost_usd: Option<f64>,
        structured_output: Option<Value>,
    },
    SessionExited {
        reason: ExitReason,
    },
    /// An event type this build does not model. Retained verbatim rather than dropped so
    /// a CLI upgrade is visible instead of silent.
    Unrecognized {
        raw: Value,
    },
    Diagnostic {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    Clean,
    Interrupted,
    /// Force-killed by the user. Distinct from `Crashed` because it must not consume a
    /// retry attempt or trigger reassignment.
    Killed,
    Crashed {
        code: Option<i32>,
    },
    StartupFailed {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: Seq,
    pub at_ms: i64,
    pub session_id: Option<SessionId>,
    pub agent_id: Option<AgentId>,
    pub task_id: Option<TaskId>,
    pub event: AgentEvent,
}
