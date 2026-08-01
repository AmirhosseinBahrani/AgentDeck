//! The validation ladder.
//!
//! Every test here is an attempt to get a bad model output past the guards. That framing is the
//! point: the design's claim is that Claude cannot cause an illegal state change, and the only
//! way to believe it is to try.

use deck_core::domain::ids::AgentId;
use deck_supervisor::contract::{Criterion, TaskContract, Verification};
use deck_supervisor::decision::*;

fn roles() -> Vec<String> {
    vec!["developer".into(), "reviewer".into()]
}

fn task(tmp_id: &str, role: &str, gate: bool) -> ProposedTask {
    ProposedTask {
        tmp_id: tmp_id.into(),
        title: format!("do {tmp_id}"),
        description: String::new(),
        role: role.into(),
        objective_gate: gate,
        contract: TaskContract::default(),
    }
}

fn plan(tasks: Vec<ProposedTask>, edges: Vec<(&str, &str)>) -> ProposedPlan {
    ProposedPlan {
        tasks,
        edges: edges
            .into_iter()
            .map(|(a, b)| ProposedEdge {
                from_tmp: a.into(),
                to_tmp: b.into(),
            })
            .collect(),
        reasoning: String::new(),
    }
}

// ---------------------------------------------------------------------------
// D1 — plan validation
// ---------------------------------------------------------------------------

#[test]
fn a_sound_plan_passes() {
    let p = plan(
        vec![task("a", "developer", true), task("b", "reviewer", false)],
        vec![("a", "b")],
    );
    assert!(validate_plan(&p, &roles(), PlanLimits::default()).is_empty());
}

#[test]
fn a_plan_naming_a_role_that_does_not_exist_is_rejected() {
    // Otherwise the task is unassignable and silently strands the run.
    let p = plan(vec![task("a", "database-engineer", true)], vec![]);
    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    assert!(faults
        .iter()
        .any(|f| matches!(f, ValidationFault::UnknownRole { .. })));
}

#[test]
fn a_plan_with_no_objective_gate_is_rejected() {
    // With no gate, objective_satisfied() can never be true and the run would never finish.
    let p = plan(vec![task("a", "developer", false)], vec![]);
    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    assert!(faults.contains(&ValidationFault::NoObjectiveGate));
}

#[test]
fn an_empty_plan_is_rejected() {
    let faults = validate_plan(&plan(vec![], vec![]), &roles(), PlanLimits::default());
    assert!(faults.contains(&ValidationFault::NoTasks));
}

#[test]
fn an_oversized_plan_is_rejected() {
    // A planner asking for 200 tasks has misunderstood the objective, and the resulting graph
    // would be unsupervisable.
    let tasks: Vec<ProposedTask> = (0..50)
        .map(|i| task(&format!("t{i}"), "developer", i == 0))
        .collect();
    let faults = validate_plan(&plan(tasks, vec![]), &roles(), PlanLimits::default());
    assert!(faults
        .iter()
        .any(|f| matches!(f, ValidationFault::TooManyTasks { .. })));
}

#[test]
fn duplicate_and_dangling_tmp_ids_are_rejected() {
    let mut p = plan(
        vec![task("a", "developer", true), task("a", "developer", false)],
        vec![("a", "ghost")],
    );
    p.tasks[1].title = "second a".into();

    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    assert!(faults
        .iter()
        .any(|f| matches!(f, ValidationFault::DuplicateTmpId(_))));
    assert!(faults
        .iter()
        .any(|f| matches!(f, ValidationFault::UnknownTmpId(_))));
}

#[test]
fn a_cycle_in_the_proposed_plan_is_caught_before_any_task_is_created() {
    // Catching it here means the graph never sees it, and no worktree is created for work that
    // could not be scheduled.
    let p = plan(
        vec![
            task("a", "developer", true),
            task("b", "developer", false),
            task("c", "developer", false),
        ],
        vec![("a", "b"), ("b", "c"), ("c", "a")],
    );
    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    assert!(faults.contains(&ValidationFault::CycleInPlan));
}

#[test]
fn a_self_edge_is_rejected() {
    let p = plan(vec![task("a", "developer", true)], vec![("a", "a")]);
    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    assert!(faults
        .iter()
        .any(|f| matches!(f, ValidationFault::SelfEdge(_))));
}

#[test]
fn an_empty_title_is_rejected() {
    let mut p = plan(vec![task("a", "developer", true)], vec![]);
    p.tasks[0].title = "   ".into();
    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    assert!(faults
        .iter()
        .any(|f| matches!(f, ValidationFault::EmptyTitle(_))));
}

#[test]
fn fault_descriptions_are_actionable_enough_to_repair_from() {
    // These are pasted into the retry prompt, so they must say what to fix, not just that
    // something is wrong.
    let p = plan(vec![task("a", "nonexistent", false)], vec![]);
    let faults = validate_plan(&p, &roles(), PlanLimits::default());
    let text = describe_faults(&faults);

    assert!(
        text.contains("nonexistent"),
        "should name the bad role: {text}"
    );
    assert!(
        text.contains("objective_gate"),
        "should name the missing gate: {text}"
    );
    assert!(text.len() > 60);
}

#[test]
fn the_plan_schema_forbids_unknown_fields() {
    // additionalProperties:false is what stops a model inventing a field we would then ignore,
    // producing a plan that looks richer than what we actually read.
    let schema = plan_schema();
    assert_eq!(schema["additionalProperties"], serde_json::json!(false));
    assert_eq!(
        schema["properties"]["tasks"]["items"]["additionalProperties"],
        serde_json::json!(false)
    );
}

// ---------------------------------------------------------------------------
// D2 — assignment
// ---------------------------------------------------------------------------

#[test]
fn an_invented_agent_id_falls_back_instead_of_being_trusted() {
    let a = AgentId::new();
    let b = AgentId::new();
    let eligible = vec![a, b];
    let choice = AssignmentChoice {
        agent_id: AgentId::new().to_string(), // not in the eligible set
        reason: "I like this one".into(),
    };

    let (chosen, source) =
        resolve_assignment(Some(&choice), &eligible, &|_| 0).expect("someone must be chosen");

    assert!(
        eligible.contains(&chosen),
        "must pick from the eligible set"
    );
    assert_eq!(
        source,
        AssignmentSource::Fallback,
        "the log must show code decided, not the model"
    );
}

#[test]
fn a_valid_choice_is_honoured() {
    let a = AgentId::new();
    let b = AgentId::new();
    let choice = AssignmentChoice {
        agent_id: b.to_string(),
        reason: "b has the right skills".into(),
    };

    let (chosen, source) = resolve_assignment(Some(&choice), &[a, b], &|_| 0).unwrap();
    assert_eq!(chosen, b);
    assert_eq!(source, AssignmentSource::Model);
}

#[test]
fn the_fallback_picks_the_least_loaded_agent() {
    // Assignment is never worth a repair round-trip: any eligible agent can do the work, and
    // stalling the loop to re-ask costs more than a suboptimal choice.
    let busy = AgentId::new();
    let idle = AgentId::new();
    let load = |id: AgentId| if id == busy { 5 } else { 0 };

    let (chosen, source) = resolve_assignment(None, &[busy, idle], &load).unwrap();
    assert_eq!(chosen, idle);
    assert_eq!(source, AssignmentSource::Fallback);
}

#[test]
fn the_fallback_is_deterministic_for_replay() {
    // The decision log doubles as a regression corpus, which only works if replaying produces
    // the same choice.
    let a = AgentId::new();
    let b = AgentId::new();
    let first = resolve_assignment(None, &[a, b], &|_| 0).unwrap().0;
    let again = resolve_assignment(None, &[b, a], &|_| 0).unwrap().0;
    assert_eq!(first, again, "tie-break must not depend on input order");
}

#[test]
fn no_eligible_agents_yields_no_assignment_rather_than_a_wrong_one() {
    assert!(resolve_assignment(None, &[], &|_| 0).is_none());
}

#[test]
fn the_assignment_schema_constrains_the_answer_to_eligible_agents() {
    let a = AgentId::new();
    let schema = assignment_schema(&[a]);
    let allowed = schema["properties"]["agent_id"]["enum"].as_array().unwrap();
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0], serde_json::json!(a.to_string()));
}

// ---------------------------------------------------------------------------
// D3 — review verdicts
// ---------------------------------------------------------------------------

fn review(verdict: Verdict, criteria: Vec<(&str, bool)>, findings: Vec<&str>) -> ReviewResult {
    ReviewResult {
        verdict,
        criteria: criteria
            .into_iter()
            .map(|(id, met)| CriterionJudgement {
                id: id.into(),
                met,
                evidence: String::new(),
            })
            .collect(),
        blocking_findings: findings.into_iter().map(String::from).collect(),
        summary: "summary".into(),
    }
}

#[test]
fn a_pass_that_contradicts_its_own_evidence_is_rejected() {
    // The most important check here: a reviewer marking a criterion unmet and still passing is
    // exactly how false success gets through.
    let ids = vec!["c1".to_string(), "c2".to_string()];
    let r = review(Verdict::Pass, vec![("c1", true), ("c2", false)], vec![]);

    let result = validate_review(&r, &ids);
    assert!(result.is_err(), "a self-contradictory pass must not stand");
    assert!(result.unwrap_err()[0].contains("contradicts"));
}

#[test]
fn a_fail_without_a_finding_is_rejected() {
    // Otherwise the developer gets a rejection with nothing to act on and repeats the work.
    let ids = vec!["c1".to_string()];
    let r = review(Verdict::Fail, vec![("c1", false)], vec![]);
    assert!(validate_review(&r, &ids).is_err());
}

#[test]
fn a_verdict_judging_a_nonexistent_criterion_is_rejected() {
    let ids = vec!["c1".to_string()];
    let r = review(
        Verdict::Pass,
        vec![("c1", true), ("invented", true)],
        vec![],
    );
    let err = validate_review(&r, &ids).unwrap_err();
    assert!(err[0].contains("invented"));
}

#[test]
fn a_coherent_verdict_is_accepted() {
    let ids = vec!["c1".to_string()];
    assert_eq!(
        validate_review(&review(Verdict::Pass, vec![("c1", true)], vec![]), &ids),
        Ok(Verdict::Pass)
    );
    assert_eq!(
        validate_review(
            &review(Verdict::Fail, vec![("c1", false)], vec!["tests fail"]),
            &ids
        ),
        Ok(Verdict::Fail)
    );
}

#[test]
fn inconclusive_is_preserved_rather_than_coerced() {
    // Silently treating "I cannot tell" as either outcome would be the worst reading of it.
    let ids = vec!["c1".to_string()];
    let r = review(Verdict::Inconclusive, vec![], vec![]);
    assert_eq!(validate_review(&r, &ids), Ok(Verdict::Inconclusive));
}

#[test]
fn the_review_schema_constrains_criterion_ids_to_the_contract() {
    let ids = vec!["c1".to_string(), "c2".to_string()];
    let schema = review_schema(&ids);
    let allowed = schema["properties"]["criteria"]["items"]["properties"]["id"]["enum"]
        .as_array()
        .unwrap();
    assert_eq!(allowed.len(), 2);
}

// ---------------------------------------------------------------------------
// D5 — failure disposition
// ---------------------------------------------------------------------------

#[test]
fn retrying_is_not_offered_once_attempts_are_exhausted() {
    // The model can only rank within the legal set, so it cannot propose a retry that the task
    // has no budget for.
    let legal = legal_failure_actions(0, true, false);
    assert!(!legal.contains(&FailureAction::RetrySameAgent));
    assert!(!legal.contains(&FailureAction::ReassignToDifferentAgent));
    assert!(legal.contains(&FailureAction::Escalate));
}

#[test]
fn permanent_failure_is_never_offered_for_an_objective_gating_task() {
    // Failing one of those autonomously would abandon the objective without asking a human.
    let legal = legal_failure_actions(0, false, true);
    assert!(!legal.contains(&FailureAction::FailPermanently));
    assert!(legal.contains(&FailureAction::Escalate));
}

#[test]
fn reassignment_requires_another_agent_to_exist() {
    let legal = legal_failure_actions(2, false, false);
    assert!(!legal.contains(&FailureAction::ReassignToDifferentAgent));
    assert!(legal.contains(&FailureAction::RetrySameAgent));
}

#[test]
fn an_illegal_proposed_action_falls_back_to_escalation() {
    // Conservative means preferring to ask a human: escalation preserves work, and a wrong
    // autonomous failure destroys it.
    let legal = legal_failure_actions(0, false, true);
    let (action, source) = resolve_failure_action(Some(FailureAction::RetrySameAgent), &legal);

    assert_eq!(action, FailureAction::Escalate);
    assert_eq!(source, AssignmentSource::Fallback);
}

#[test]
fn a_legal_proposed_action_is_honoured() {
    let legal = legal_failure_actions(2, true, false);
    let (action, source) =
        resolve_failure_action(Some(FailureAction::ReassignToDifferentAgent), &legal);
    assert_eq!(action, FailureAction::ReassignToDifferentAgent);
    assert_eq!(source, AssignmentSource::Model);
}

#[test]
fn no_proposal_still_yields_a_conservative_action() {
    let legal = legal_failure_actions(1, true, false);
    let (action, source) = resolve_failure_action(None, &legal);
    assert_eq!(action, FailureAction::Escalate);
    assert_eq!(source, AssignmentSource::Fallback);
}

// ---------------------------------------------------------------------------
// Contract integration
// ---------------------------------------------------------------------------

#[test]
fn a_planned_task_carrying_only_judgment_criteria_is_repaired_before_use() {
    // The planner is allowed to omit verification; the supervisor is not allowed to accept a
    // contract that cannot be checked.
    let mut planned = task("a", "developer", true);
    planned.contract = TaskContract {
        version: 1,
        acceptance_criteria: vec![Criterion {
            id: "looks-good".into(),
            text: "the code is nice".into(),
            verify: Verification::Judgment {
                rubric: "nice?".into(),
            },
        }],
        constraints: vec![],
        deliverables: vec![],
        definition_of_done: "done".into(),
    };

    assert!(!planned.contract.has_executable_criterion());
    let repairs =
        deck_supervisor::contract::validate_and_repair(&mut planned.contract, Some("cargo test"));
    assert_eq!(repairs.len(), 1);
    assert!(planned.contract.has_executable_criterion());
}
