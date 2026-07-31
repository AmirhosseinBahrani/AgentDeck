//! Structured worker reporting.
//!
//! Workers signal through tools rather than prose. A prompt asking for periodic JSON is
//! unenforceable, and parsing progress out of a transcript is exactly what this design rejects —
//! a tool call is observable, typed and timestamped, and `claim_task_done` being a *tool* is what
//! makes it the only legal completion path.
//!
//! The tools are exposed to an agent by a small MCP server that the CLI spawns per session. That
//! server is a separate process, so it reaches the app over a unix socket. The wire types live
//! here, shared by both sides, because two copies of a protocol drift.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Env var carrying the socket path into the spawned MCP server.
pub const SOCKET_ENV: &str = "AGENTDECK_REPORT_SOCKET";
/// Env var carrying the task the agent is working on, so a report cannot be misattributed.
pub const TASK_ENV: &str = "AGENTDECK_TASK_ID";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum WorkerReport {
    /// Periodic progress. Advisory: the supervisor also derives telemetry the worker cannot
    /// influence, such as diff churn and tool-call rate.
    ReportProgress {
        summary: String,
        #[serde(default)]
        completed_work: Vec<String>,
        #[serde(default)]
        remaining_work: Vec<String>,
        #[serde(default)]
        blockers: Vec<String>,
    },
    /// The only legal way to finish. A session that exits without this has failed, not completed.
    ClaimTaskDone { summary: String },
    /// The worker cannot proceed and needs a human or another task.
    RaiseBlocker { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportEnvelope {
    pub task_id: String,
    #[serde(flatten)]
    pub report: WorkerReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportAck {
    pub accepted: bool,
    /// Shown to the agent as the tool result. On rejection it must explain why, or the model will
    /// simply call the tool again.
    pub message: String,
}

impl ReportAck {
    pub fn accepted(message: impl Into<String>) -> Self {
        Self {
            accepted: true,
            message: message.into(),
        }
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        Self {
            accepted: false,
            message: message.into(),
        }
    }
}

/// The tool definitions advertised to an agent.
///
/// Descriptions are deliberately blunt about consequences. A worker that does not know
/// `claim_task_done` is mandatory will simply stop when it thinks it is finished, and be recorded
/// as having failed.
pub fn tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "report_progress",
            "description":
                "Report what you have done so far and what remains. Call this every few minutes \
                 during long work so the supervisor can tell you are making progress rather than \
                 stuck.",
            "inputSchema": {
                "type": "object",
                "required": ["summary"],
                "properties": {
                    "summary": { "type": "string" },
                    "completed_work": { "type": "array", "items": { "type": "string" } },
                    "remaining_work": { "type": "array", "items": { "type": "string" } },
                    "blockers": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "claim_task_done",
            "description":
                "Declare the task complete. This is the ONLY way to finish: if your session ends \
                 without calling it, the task is recorded as failed. Your acceptance criteria are \
                 then verified by the supervisor running them itself, so claiming completion \
                 before they pass will be rejected.",
            "inputSchema": {
                "type": "object",
                "required": ["summary"],
                "properties": { "summary": { "type": "string" } }
            }
        },
        {
            "name": "raise_blocker",
            "description":
                "Report that you cannot proceed and need a decision or another task to finish \
                 first. Prefer this over guessing or working around the problem.",
            "inputSchema": {
                "type": "object",
                "required": ["reason"],
                "properties": { "reason": { "type": "string" } }
            }
        }
    ])
}

/// Builds the `--mcp-config` value pointing the CLI at the reporting server.
///
/// `strict_mcp_config` is passed separately at spawn, so this is the only MCP server an agent
/// sees — it cannot reach the developer's own servers.
pub fn mcp_config(server_binary: &Path, socket: &Path, task_id: &str) -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            "agentdeck": {
                "command": server_binary.to_string_lossy(),
                "args": [],
                "env": {
                    SOCKET_ENV: socket.to_string_lossy(),
                    TASK_ENV: task_id,
                }
            }
        }
    })
}

/// Maximum usable unix socket path length.
///
/// `sockaddr_un.sun_path` is 104 bytes on macOS and 108 on Linux; the lower bound applies. This is
/// the kind of limit that passes in a shallow development directory and fails once the path is a
/// little deeper, so names here are kept deliberately short and the length is checked rather than
/// assumed.
pub const MAX_SOCKET_PATH: usize = 100;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error(
    "socket path is {length} bytes, over the {MAX_SOCKET_PATH}-byte limit: {path}. \
     Use a shorter runtime directory."
)]
pub struct SocketPathTooLong {
    pub path: String,
    pub length: usize,
}

/// Where a session's report socket lives.
///
/// One socket per session rather than one shared: a report then cannot be attributed to the wrong
/// agent even if a stale server outlives its parent. The name uses a short prefix of the session
/// id rather than the whole uuid, because the full path must fit in `MAX_SOCKET_PATH`.
pub fn socket_path(runtime_dir: &Path, session_id: &str) -> PathBuf {
    let short: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(10)
        .collect();
    runtime_dir.join(format!("ad-{short}.sock"))
}

/// Checks a socket path fits, so the failure is reported where it can be understood rather than as
/// an opaque `InvalidInput` from `bind`.
pub fn check_socket_path(path: &Path) -> Result<(), SocketPathTooLong> {
    let length = path.as_os_str().len();
    if length > MAX_SOCKET_PATH {
        return Err(SocketPathTooLong {
            path: path.display().to_string(),
            length,
        });
    }
    Ok(())
}
