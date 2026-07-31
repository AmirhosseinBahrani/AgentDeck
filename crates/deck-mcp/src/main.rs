//! The MCP server AgentDeck gives each agent.
//!
//! Spawned by the `claude` CLI per session, speaking JSON-RPC over stdio. It exposes three tools
//! and forwards each call to AgentDeck over a unix socket, so the app — not this process — decides
//! whether a report is accepted.
//!
//! Deliberately thin. It holds no state and makes no decisions: a bug here should be able to lose
//! a report, never to fabricate one or to accept a completion the supervisor would have rejected.

use deck_core::reporting::{
    tool_definitions, ReportAck, ReportEnvelope, WorkerReport, SOCKET_ENV, TASK_ENV,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

/// MCP revision this speaks. Echoed back at initialize; a client asking for a different one still
/// works, since the three methods used here are stable across revisions.
const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() {
    let socket = std::env::var(SOCKET_ENV).unwrap_or_default();
    let task_id = std::env::var(TASK_ENV).unwrap_or_default();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            // Unparseable input is the client's problem; staying alive is better than dying and
            // taking the agent's tools with us.
            continue;
        };

        // Notifications have no id and must not be answered.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let response = match method {
            "initialize" => success(
                id,
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "agentdeck", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "tools/list" => success(id, serde_json::json!({ "tools": tool_definitions() })),
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or_default();
                handle_tool_call(id, &params, &socket, &task_id)
            }
            "ping" => success(id, serde_json::json!({})),
            _ => error(id, -32601, &format!("method not found: {method}")),
        };

        if writeln!(out, "{response}").is_err() || out.flush().is_err() {
            break;
        }
    }
}

fn handle_tool_call(
    id: serde_json::Value,
    params: &serde_json::Value,
    socket: &str,
    task_id: &str,
) -> serde_json::Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let report = match parse_report(name, &arguments) {
        Ok(report) => report,
        Err(message) => return tool_error(id, &message),
    };

    let envelope = ReportEnvelope {
        task_id: task_id.to_string(),
        report,
    };

    match send(socket, &envelope) {
        Ok(ack) if ack.accepted => tool_text(id, &ack.message),
        // A rejection is reported as a tool error so the model treats it as a refusal to act on,
        // rather than as confirmation with an odd message attached.
        Ok(ack) => tool_error(id, &ack.message),
        Err(e) => tool_error(
            id,
            &format!("AgentDeck did not accept the report ({e}); it was not recorded."),
        ),
    }
}

fn parse_report(name: &str, arguments: &serde_json::Value) -> Result<WorkerReport, String> {
    let text = |key: &str| -> Result<String, String> {
        arguments
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("{name} requires a non-empty {key}"))
    };
    let list = |key: &str| -> Vec<String> {
        arguments
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    match name {
        "report_progress" => Ok(WorkerReport::ReportProgress {
            summary: text("summary")?,
            completed_work: list("completed_work"),
            remaining_work: list("remaining_work"),
            blockers: list("blockers"),
        }),
        "claim_task_done" => Ok(WorkerReport::ClaimTaskDone {
            summary: text("summary")?,
        }),
        "raise_blocker" => Ok(WorkerReport::RaiseBlocker {
            reason: text("reason")?,
        }),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn send(socket: &str, envelope: &ReportEnvelope) -> Result<ReportAck, String> {
    if socket.is_empty() {
        return Err("no report socket configured".into());
    }

    let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(envelope).map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).map_err(|e| e.to_string())?;

    serde_json::from_str(response.trim()).map_err(|e| e.to_string())
}

fn success(id: serde_json::Value, result: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: serde_json::Value, code: i32, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id,
        "error": { "code": code, "message": message }
    })
}

fn tool_text(id: serde_json::Value, text: &str) -> serde_json::Value {
    success(
        id,
        serde_json::json!({ "content": [{ "type": "text", "text": text }] }),
    )
}

/// A tool-level failure, reported inside `result` with `isError` rather than as a JSON-RPC error.
/// That is what makes the model see it as the tool refusing rather than the server breaking.
fn tool_error(id: serde_json::Value, text: &str) -> serde_json::Value {
    success(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": text }],
            "isError": true
        }),
    )
}
