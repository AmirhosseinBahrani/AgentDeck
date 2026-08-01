//! Consults Claude through a one-shot CLI invocation per decision.
//!
//! Lives here rather than in the app because it is a [`Planner`](crate::planner::Planner)
//! implementation with no Tauri dependency — which is also what lets an integration test drive a
//! real CLI call and catch a response shape we cannot read.

use crate::planner::{ModelResponse, PlannerError};
use std::path::PathBuf;

/// Consults Claude via a one-shot CLI invocation per decision.
///
/// Settings are pinned so the supervisor never inherits the developer's plugins or hooks, and
/// `--json-schema` does the shape validation before our own ladder runs.
///
/// Note what is *absent*: `--verbose`. It is mandatory with `stream-json`, which is why workers
/// pass it, but with plain `json` it changes the response from the result object to an array of
/// every event — and that broke planning completely while looking like the app doing nothing.
///
/// Tools are disabled outright. The supervisor reasons; it does not act. Anything it needs to know
/// about the repository is assembled into the prompt by code, which is what keeps its inputs
/// inspectable and its decisions replayable.
pub struct CliPlanner {
    program: String,
    cwd: PathBuf,
    model: Option<String>,
}

impl CliPlanner {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            program: "claude".into(),
            cwd,
            model: None,
        }
    }
}

#[async_trait::async_trait]
impl crate::planner::Planner for CliPlanner {
    async fn call(&self, call: crate::planner::ModelCall) -> Result<ModelResponse, PlannerError> {
        let schema =
            serde_json::to_string(&call.schema).map_err(|e| PlannerError::Failed(e.to_string()))?;

        let mut cmd = tokio::process::Command::new(&self.program);
        cmd.arg("-p")
            .arg(&call.prompt)
            .args(["--output-format", "json"])
            // No --verbose. It is mandatory for `stream-json`, which is why workers pass it, but
            // with plain `json` it changes the *shape* of the output from the result object to
            // an array of every event — and reading `structured_output` off an array yields
            // nothing, which failed every plan and meant no task was ever created.
            .args(["--json-schema", &schema])
            .args(["--max-budget-usd", &call.max_budget_usd.to_string()])
            // No plugins, hooks or MCP servers from the developer's environment.
            .args(["--setting-sources", ""])
            .arg("--strict-mcp-config")
            // The supervisor decides; it never touches the repository itself.
            .args(["--tools", ""])
            .args(["--permission-mode", "default"])
            .current_dir(&self.cwd)
            .kill_on_drop(true);

        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| PlannerError::Failed(e.to_string()))?;

        if !output.status.success() {
            return Err(PlannerError::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        extract_response(&stdout)
    }
}

/// Pulls the model's answer out of whatever `--output-format json` produced.
///
/// Tolerant of both shapes the CLI emits: the result object on its own, and the array of every
/// event that `--verbose` produces. We no longer pass `--verbose` here, but reading only one
/// shape is what silently broke planning once already — and the failure was invisible, because
/// an unreadable answer is indistinguishable from a model that refused to answer. Accepting both
/// costs a few lines and removes that whole class of outage.
fn extract_response(stdout: &str) -> Result<ModelResponse, PlannerError> {
    let parsed: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| PlannerError::Parse(e.to_string()))?;

    let result = match &parsed {
        serde_json::Value::Array(events) => events
            .iter()
            .rev()
            .find(|event| event.get("type").and_then(|t| t.as_str()) == Some("result"))
            .ok_or_else(|| PlannerError::Parse("no result event in the response".into()))?,
        object => object,
    };

    // `structured_output` is present only when --json-schema validated successfully. Its absence
    // means the model never produced a conforming answer, which the caller treats as an unusable
    // response rather than guessing at the prose.
    let structured = result
        .get("structured_output")
        .cloned()
        .ok_or(PlannerError::NoStructuredOutput)?;

    Ok(ModelResponse {
        structured,
        cost_usd: result.get("total_cost_usd").and_then(|c| c.as_f64()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `--output-format json` produces on its own.
    const RESULT_OBJECT: &str = r#"{
        "type": "result", "subtype": "success", "is_error": false,
        "total_cost_usd": 0.0632725,
        "structured_output": {"tasks": [], "reasoning": "none"}
    }"#;

    /// What adding --verbose produces instead: every event, as an array. This is what the
    /// planner actually received, and reading it as an object is why no plan ever succeeded.
    const EVENT_ARRAY: &str = r#"[
        {"type": "system", "subtype": "init", "session_id": "abc"},
        {"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}},
        {"type": "result", "subtype": "success", "total_cost_usd": 0.0632725,
         "structured_output": {"tasks": [], "reasoning": "none"}}
    ]"#;

    #[test]
    fn the_plain_result_object_is_read() {
        let response = extract_response(RESULT_OBJECT).expect("should parse");
        assert_eq!(response.cost_usd, Some(0.0632725));
        assert!(response.structured.get("tasks").is_some());
    }

    #[test]
    fn the_verbose_event_array_is_also_read() {
        // The regression this exists for: an array yielded no structured output, every plan
        // failed, no task was ever created, and the run looked like it simply did nothing.
        let response = extract_response(EVENT_ARRAY).expect("should parse");
        assert_eq!(response.cost_usd, Some(0.0632725));
        assert!(response.structured.get("tasks").is_some());
    }

    #[test]
    fn a_response_without_structured_output_is_reported_as_such() {
        // Distinct from a parse failure: the model answered, but not in the required shape, and
        // the caller has a repair round-trip for exactly that.
        let err = extract_response(r#"{"type":"result","total_cost_usd":0.1}"#).unwrap_err();
        assert!(
            matches!(err, crate::planner::PlannerError::NoStructuredOutput),
            "got {err:?}"
        );
    }

    #[test]
    fn an_array_with_no_result_event_is_a_parse_error() {
        let err = extract_response(r#"[{"type":"system"}]"#).unwrap_err();
        assert!(
            matches!(err, crate::planner::PlannerError::Parse(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn output_that_is_not_json_is_a_parse_error() {
        assert!(extract_response("command not found").is_err());
    }
}
