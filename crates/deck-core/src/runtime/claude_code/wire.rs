//! Serde model for the `claude` CLI's NDJSON stream, verified against v2.1.153.
//!
//! Forward compatibility is the whole game here: the CLI ships frequently and already
//! emits content-block kinds and event types that are not in this file. An unrecognized
//! shape must degrade to `Unknown` and be passed through — never fail a session.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One line of CLI stdout.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Outbound {
    Known(KnownOutbound),
    /// Unrecognized event type or malformed-but-valid JSON. Counted and passed to the
    /// UI verbatim so a CLI upgrade degrades gracefully instead of killing the session.
    Unknown(Value),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownOutbound {
    /// `subtype` is an open set: observed `init`, `hook_started`, `hook_response`,
    /// `hook_progress`, `compact_boundary`.
    System {
        subtype: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(flatten)]
        rest: Map<String, Value>,
    },
    Assistant {
        message: Value,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        parent_tool_use_id: Option<String>,
    },
    /// Both replayed user input and tool results arrive under this type.
    User {
        message: Value,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Token-level deltas, only with `--include-partial-messages`. Never persisted.
    StreamEvent {
        event: Value,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Proactive, structured rate-limit signal. Strictly better than sniffing errors:
    /// it carries a reset timestamp, so the scheduler can back off globally and the UI
    /// can say when work resumes.
    RateLimitEvent {
        rate_limit_info: RateLimitInfo,
        #[serde(default)]
        session_id: Option<String>,
    },
    /// Emitted once per *turn*, not once per process.
    Result(Box<TurnResult>),
    ControlRequest {
        request_id: String,
        request: ControlRequestBody,
    },
    ControlResponse {
        response: Value,
    },
    ControlCancelRequest {
        request_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub status: String,
    /// Unix seconds. Surface this as "resuming ~HH:MM".
    #[serde(default, rename = "resetsAt")]
    pub resets_at: Option<i64>,
    #[serde(default, rename = "rateLimitType")]
    pub rate_limit_type: Option<String>,
    #[serde(default, rename = "overageStatus")]
    pub overage_status: Option<String>,
    #[serde(default, rename = "isUsingOverage")]
    pub is_using_overage: Option<bool>,
}

/// Per-turn terminal event. `total_cost_usd` is per-turn, so accounting must SUM
/// these across the session rather than taking the latest value.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnResult {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub api_error_status: Option<Value>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub num_turns: Option<u32>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<Value>,
    /// Present only with `--json-schema`, and only on the final result of the run.
    #[serde(default)]
    pub structured_output: Option<Value>,
    #[serde(default)]
    pub permission_denials: Vec<PermissionDenial>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionDenial {
    pub tool_name: String,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub tool_input: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlRequestBody {
    /// Sent when a tool decision resolves to "ask". Requires `--permission-prompt-tool stdio`;
    /// without it the CLI short-circuits to deny and only an `is_error` tool_result is seen.
    CanUseTool(Box<CanUseTool>),
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CanUseTool {
    pub tool_name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    pub input: Value,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    /// The CLI classifies *why* it is asking, e.g. `"workingDir"`. This means the broker
    /// applies policy to a pre-classified decision instead of reimplementing path containment.
    #[serde(default)]
    pub decision_reason: Option<String>,
    #[serde(default)]
    pub decision_reason_type: Option<String>,
    #[serde(default)]
    pub blocked_path: Option<String>,
    /// Structured options (addRules / addDirectories). These map straight onto the UI's
    /// typed escalation options, so the frontend never parses prose.
    #[serde(default)]
    pub permission_suggestions: Vec<Value>,
}

// ---------------------------------------------------------------------------
// Host -> CLI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inbound {
    User {
        message: UserMessage,
    },
    ControlRequest {
        request_id: String,
        request: Value,
    },
    ControlResponse {
        response: ControlResponseEnvelope,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct UserMessage {
    pub role: &'static str,
    pub content: Vec<ContentBlock>,
}

impl UserMessage {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            role: "user",
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ControlResponseEnvelope {
    pub subtype: &'static str,
    pub request_id: String,
    pub response: Value,
}

#[derive(Debug, Clone)]
pub enum PermissionDecision {
    Allow { updated_input: Value },
    Deny { message: String },
}

impl Inbound {
    pub fn user_text(text: impl Into<String>) -> Self {
        Inbound::User {
            message: UserMessage::text(text),
        }
    }

    /// Answer a `can_use_tool` control request.
    pub fn permission(request_id: String, decision: PermissionDecision) -> Self {
        let response = match decision {
            PermissionDecision::Allow { updated_input } => serde_json::json!({
                "behavior": "allow",
                "updatedInput": updated_input,
            }),
            // Always carry a message: a bare denial reads to the agent as a tool
            // malfunction, and it will usually retry the same call.
            PermissionDecision::Deny { message } => serde_json::json!({
                "behavior": "deny",
                "message": message,
            }),
        };
        Inbound::ControlResponse {
            response: ControlResponseEnvelope {
                subtype: "success",
                request_id,
                response,
            },
        }
    }

    pub fn to_ndjson_line(&self) -> Result<String, serde_json::Error> {
        Ok(format!("{}\n", serde_json::to_string(self)?))
    }
}
