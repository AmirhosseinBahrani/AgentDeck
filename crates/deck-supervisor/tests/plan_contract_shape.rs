//! The contract the planner is asked for, and what happens when it gets it wrong.
//!
//! Both of these come from a real failure: a run died with
//! `invalid type: string "Run: test -s greeting.txt …", expected a sequence`.
//! The schema sent to the CLI described the contract as a bare `{"type": "object"}`, so
//! `--json-schema` — the thing that is supposed to do shape validation before our own ladder
//! runs — validated nothing at all. The model invented a shape, our types rejected it, and one
//! malformed field discarded an entire plan.

use deck_supervisor::contract::TaskContract;
use deck_supervisor::decision::{plan_schema, ProposedPlan};

#[test]
fn the_schema_describes_the_contract_rather_than_accepting_any_object() {
    let schema = plan_schema();
    let contract = &schema["properties"]["tasks"]["items"]["properties"]["contract"];

    assert!(
        contract["properties"]["acceptance_criteria"].is_object(),
        "the CLI cannot validate a contract it was never given the shape of"
    );
    assert_eq!(
        contract["properties"]["acceptance_criteria"]["type"], "array",
        "acceptance_criteria is a list; the failure was the model sending a string"
    );
    assert_eq!(contract["properties"]["deliverables"]["type"], "array");
    assert_eq!(contract["properties"]["constraints"]["type"], "array");
    assert_eq!(contract["additionalProperties"], false);
}

#[test]
fn the_verification_shape_is_specified_so_a_criterion_can_be_run() {
    // A criterion whose `verify` is unconstrained is a criterion the gate cannot execute, which
    // silently turns the deterministic check into a judgement call.
    let schema = plan_schema();
    let verify = &schema["properties"]["tasks"]["items"]["properties"]["contract"]["properties"]
        ["acceptance_criteria"]["items"]["properties"]["verify"];

    let kinds = verify["properties"]["type"]["enum"]
        .as_array()
        .expect("verification kinds must be enumerated");
    assert!(kinds.contains(&serde_json::json!("command")));
    assert!(kinds.contains(&serde_json::json!("files_exist")));
    assert!(kinds.contains(&serde_json::json!("judgment")));
}

#[test]
fn a_task_survives_a_contract_the_model_shaped_wrongly() {
    // The exact payload that killed a run: a string where a list belongs.
    let raw = serde_json::json!({
        "tasks": [{
            "tmp_id": "a",
            "title": "Write a greeting",
            "role": "developer",
            "contract": {
                "acceptance_criteria": "Run: test -s greeting.txt && grep -qi 'hi' greeting.txt",
                "definition_of_done": "done"
            }
        }],
        "edges": []
    });

    let plan: ProposedPlan =
        serde_json::from_value(raw).expect("a bad contract must not fail the plan");
    assert_eq!(plan.tasks.len(), 1, "the task itself is still usable");
    assert_eq!(plan.tasks[0].title, "Write a greeting");
    assert_eq!(
        plan.tasks[0].contract,
        TaskContract::default(),
        "the unusable contract falls back to empty, which validate_and_repair then fills"
    );
}

#[test]
fn a_well_formed_contract_still_parses_intact() {
    // The fallback must not be swallowing good contracts too.
    let raw = serde_json::json!({
        "tasks": [{
            "tmp_id": "a", "title": "t", "role": "developer",
            "contract": {
                "definition_of_done": "it works",
                "acceptance_criteria": [{
                    "id": "c1", "text": "tests pass",
                    "verify": { "type": "command", "cmd": "cargo test" }
                }]
            }
        }],
        "edges": []
    });

    let plan: ProposedPlan = serde_json::from_value(raw).expect("parse");
    assert_eq!(plan.tasks[0].contract.acceptance_criteria.len(), 1);
    assert!(plan.tasks[0].contract.has_executable_criterion());
}
