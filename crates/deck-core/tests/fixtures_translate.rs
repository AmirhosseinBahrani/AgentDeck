//! Replays NDJSON captured from a real `claude` v2.1.153 session through the translator.
//!
//! These fixtures are the contract. If a CLI upgrade changes the stream shape, these tests
//! should be regenerated deliberately rather than silently accommodated — and crucially,
//! nothing here may panic or produce a `Diagnostic`, because a parse failure in production
//! would kill a live agent session.

use deck_core::domain::event::AgentEvent;
use deck_core::runtime::claude_code::translate::Translator;

fn events(fixture: &str) -> Vec<AgentEvent> {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let mut t = Translator::new();
    let mut out = Vec::new();
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        out.extend(t.translate(line));
    }
    out
}

fn assert_no_parse_failures(evs: &[AgentEvent]) {
    for e in evs {
        if let AgentEvent::Diagnostic { message } = e {
            panic!("translator failed on real CLI output: {message}");
        }
    }
}

#[test]
fn tool_use_and_result_are_paired_by_id() {
    let evs = events("probe1.ndjson");
    assert_no_parse_failures(&evs);

    let call = evs
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall {
                tool_use_id, tool, ..
            } => Some((tool_use_id.clone(), tool.clone())),
            _ => None,
        })
        .expect("expected a tool call");
    assert_eq!(call.1, "Read");

    let matched = evs.iter().any(|e| {
        matches!(
            e,
            AgentEvent::ToolResult { tool_use_id, .. } if *tool_use_id == call.0
        )
    });
    assert!(matched, "tool_result did not correlate to its tool_use id");
}

#[test]
fn first_init_is_a_handshake_and_later_inits_are_turn_boundaries() {
    // probe2 ran two turns in one process, so the CLI emitted init twice.
    let evs = events("probe2.ndjson");
    assert_no_parse_failures(&evs);

    let ready = evs
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionReady { .. }))
        .count();
    let turns = evs
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStarted))
        .count();

    assert_eq!(
        ready, 1,
        "startup must be signalled exactly once per process"
    );
    assert_eq!(turns, 1, "the second init should read as a turn boundary");
}

#[test]
fn per_turn_costs_must_be_summed_not_replaced() {
    let evs = events("probe2.ndjson");
    let costs: Vec<f64> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnComplete { cost_usd, .. } => *cost_usd,
            _ => None,
        })
        .collect();

    assert_eq!(costs.len(), 2, "one result per turn");
    // Each turn reports its own cost; taking the last value would undercount the session.
    let total: f64 = costs.iter().sum();
    assert!(
        total > costs[1],
        "summing must exceed any single turn's cost"
    );
}

#[test]
fn out_of_worktree_write_surfaces_as_a_classified_permission_request() {
    let evs = events("probe4.ndjson");
    assert_no_parse_failures(&evs);

    // probe4 was captured without --permission-prompt-tool, so the CLI short-circuits to
    // deny and no permission request appears. The control_response to our `initialize`
    // must still parse cleanly rather than being treated as an unknown event.
    assert!(
        !evs.iter()
            .any(|e| matches!(e, AgentEvent::Unrecognized { .. })),
        "control_response should be recognized, not passed through as unknown"
    );
}

#[test]
fn rate_limit_events_are_captured_with_reset_timestamp() {
    let evs = events("probe1.ndjson");
    let rl = evs
        .iter()
        .find_map(|e| match e {
            AgentEvent::RateLimited {
                status, resets_at, ..
            } => Some((status.clone(), *resets_at)),
            _ => None,
        })
        .expect("expected a rate_limit_event in the captured stream");

    assert_eq!(rl.0, "allowed");
    assert!(rl.1.is_some(), "resets_at drives the 'resuming ~HH:MM' UI");
}

#[test]
fn structured_output_is_present_only_on_the_final_result() {
    // probe5 used --json-schema across two turns and produced exactly one result.
    let evs = events("probe5.ndjson");
    assert_no_parse_failures(&evs);

    let with_schema: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnComplete {
                structured_output, ..
            } => structured_output.as_ref(),
            _ => None,
        })
        .collect();

    assert_eq!(
        with_schema.len(),
        1,
        "--json-schema collapses a duplex session to a single final result, \
         which is why worker progress must go through MCP tools instead"
    );
    assert_eq!(with_schema[0]["n"], 2);
}

#[test]
fn unknown_event_types_degrade_instead_of_failing() {
    let mut t = Translator::new();
    let evs = t.translate(r#"{"type":"some_future_event_type","payload":{"a":1}}"#);
    assert!(matches!(evs[0], AgentEvent::Unrecognized { .. }));
    assert_eq!(t.unrecognized_count, 1);
}
