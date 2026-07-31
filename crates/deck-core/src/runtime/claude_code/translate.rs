//! Pure `wire::Outbound` -> `AgentEvent` translation. No I/O, so it is table-testable
//! against the captured NDJSON fixtures.

use crate::domain::event::AgentEvent;
use crate::runtime::claude_code::wire::{ControlRequestBody, KnownOutbound, Outbound};
use serde_json::Value;

/// Tracks the small amount of state needed to interpret the stream, namely that
/// `system/init` is re-emitted every turn and only the first one is a handshake.
#[derive(Debug, Default)]
pub struct Translator {
    seen_init: bool,
    pub unrecognized_count: u64,
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn translate(&mut self, line: &str) -> Vec<AgentEvent> {
        let parsed: Outbound = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return vec![AgentEvent::Diagnostic {
                    message: format!("unparseable stream line: {e}"),
                }]
            }
        };

        match parsed {
            Outbound::Unknown(raw) => {
                self.unrecognized_count += 1;
                vec![AgentEvent::Unrecognized { raw }]
            }
            Outbound::Known(k) => self.translate_known(k),
        }
    }

    fn translate_known(&mut self, k: KnownOutbound) -> Vec<AgentEvent> {
        match k {
            KnownOutbound::System { subtype, rest, .. } => match subtype.as_str() {
                "init" if !self.seen_init => {
                    self.seen_init = true;
                    vec![AgentEvent::SessionReady {
                        cwd: rest
                            .get("cwd")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        model: rest.get("model").and_then(Value::as_str).map(str::to_string),
                        tools: rest
                            .get("tools")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }]
                }
                // Subsequent inits mark turn boundaries, not startup.
                "init" => vec![AgentEvent::TurnStarted],
                _ => vec![],
            },

            KnownOutbound::Assistant { message, .. } => blocks(&message)
                .iter()
                .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") => Some(AgentEvent::ToolCall {
                        tool_use_id: str_at(b, "id"),
                        tool: str_at(b, "name"),
                        input: b.get("input").cloned().unwrap_or(Value::Null),
                    }),
                    // `thinking` is intentionally dropped: it is large, not user-facing
                    // in the dashboard, and never something we make decisions on.
                    Some("thinking") => None,
                    _ => Some(AgentEvent::Message {
                        role: "assistant".into(),
                        content: (*b).clone(),
                    }),
                })
                .collect(),

            KnownOutbound::User { message, .. } => blocks(&message)
                .iter()
                .filter_map(|b| match b.get("type").and_then(Value::as_str) {
                    Some("tool_result") => Some(AgentEvent::ToolResult {
                        tool_use_id: str_at(b, "tool_use_id"),
                        output: b.get("content").cloned().unwrap_or(Value::Null),
                        is_error: b
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }),
                    _ => None,
                })
                .collect(),

            KnownOutbound::RateLimitEvent {
                rate_limit_info: i, ..
            } => vec![AgentEvent::RateLimited {
                status: i.status,
                resets_at: i.resets_at,
                limit_type: i.rate_limit_type,
            }],

            KnownOutbound::Result(r) => vec![AgentEvent::TurnComplete {
                subtype: r.subtype.clone(),
                is_error: r.is_error,
                cost_usd: r.total_cost_usd,
                structured_output: r.structured_output.clone(),
            }],

            KnownOutbound::ControlRequest {
                request_id,
                request,
            } => match request {
                ControlRequestBody::CanUseTool(c) => vec![AgentEvent::PermissionRequest {
                    request_id,
                    tool: c.tool_name,
                    input: c.input,
                    reason_type: c.decision_reason_type,
                    blocked_path: c.blocked_path,
                    suggestions: c.permission_suggestions,
                }],
                ControlRequestBody::Other => vec![],
            },

            // Deltas are handled on a separate channel and never persisted.
            KnownOutbound::StreamEvent { .. }
            | KnownOutbound::ControlResponse { .. }
            | KnownOutbound::ControlCancelRequest { .. } => vec![],
        }
    }
}

fn blocks(message: &Value) -> Vec<&Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
