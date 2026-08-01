//! Bounded decision points and the validation ladder.
//!
//! Claude is consulted only where judgement is genuinely required, and never trusted with the
//! consequence. Every output passes a ladder before it may touch state:
//!
//! 1. **Shape** — handled upstream by the CLI's `--json-schema`.
//! 2. **Referential integrity** — every id it names must exist.
//! 3. **Whitelist** — the answer must be one of a set *this code* computed. A model cannot
//!    invent an agent to assign to, or an action that is not currently legal.
//! 4. **Size caps** — a plan cannot be arbitrarily large.
//! 5. **Invariants** — no cycles, no reopening terminal tasks.
//! 6. **Semantic requirements** — a code task must carry executable verification.
//!
//! On failure the caller gets one repair round-trip with the validator's complaints attached,
//! then a **deterministic fallback**. The model is never looped: an unbounded repair loop is how
//! this design would burn a rate limit while appearing to work.

use crate::contract::TaskContract;
use deck_core::domain::ids::{AgentId, TaskId};
use serde::{Deserialize, Serialize};

/// Caps on a single planning response. Deliberately tight — a planner that wants 200 tasks has
/// misunderstood the objective, and accepting it would produce a graph nobody can supervise.
#[derive(Debug, Clone, Copy)]
pub struct PlanLimits {
    pub max_tasks: usize,
    pub max_edges: usize,
    pub max_title_len: usize,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_tasks: 20,
            max_edges: 40,
            max_title_len: 120,
        }
    }
}

// ---------------------------------------------------------------------------
// D1 — decompose an objective
// ---------------------------------------------------------------------------

/// A planned task as the model proposes it. `tmp_id` is the model's own label; real `TaskId`s are
/// minted by code after validation, so a model cannot address an existing task by guessing an id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedTask {
    pub tmp_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub role: String,
    #[serde(default)]
    pub objective_gate: bool,
    /// Defaulted on *any* deserialization failure, not just absence.
    ///
    /// A contract the model shaped wrongly used to fail the whole response, so one bad field in
    /// one task discarded an entire plan and escalated a run that was otherwise fine. The
    /// contract is the one part of a task code can repair by itself — `validate_and_repair`
    /// injects the project's test command when verification is missing — so falling back to an
    /// empty contract loses far less than losing the plan, and the repair is recorded either way.
    #[serde(default, deserialize_with = "contract_or_default")]
    pub contract: TaskContract,
}

fn contract_or_default<'de, D>(deserializer: D) -> Result<TaskContract, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedEdge {
    pub from_tmp: String,
    pub to_tmp: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposedPlan {
    pub tasks: Vec<ProposedTask>,
    #[serde(default)]
    pub edges: Vec<ProposedEdge>,
    #[serde(default)]
    pub reasoning: String,
}

/// JSON Schema handed to the CLI so the shape is enforced before we see it.
/// The shape of a task contract, as the model must produce it.
///
/// Split out because `json!` hits its recursion limit if the whole plan is one literal — and
/// because this is the part worth reading on its own: it is the difference between the CLI
/// validating the contract and the CLI validating nothing.
fn contract_schema() -> serde_json::Value {
    let verification = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["type"],
        "properties": {
            "type": { "type": "string", "enum": ["command", "files_exist", "judgment"] },
            "cmd": { "type": "string" },
            "cwd_rel": { "type": "string" },
            "expect_exit_zero": { "type": "boolean" },
            "globs": { "type": "array", "items": { "type": "string" } },
            "rubric": { "type": "string" }
        }
    });

    let criterion = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "text", "verify"],
        "properties": {
            "id": { "type": "string" },
            "text": { "type": "string" },
            "verify": verification
        }
    });

    let constraint = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["type"],
        "properties": {
            "type": { "type": "string", "enum": ["paths_forbidden", "max_diff_lines", "textual"] },
            "globs": { "type": "array", "items": { "type": "string" } },
            "max": { "type": "integer" },
            "text": { "type": "string" }
        }
    });

    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["acceptance_criteria", "definition_of_done"],
        "properties": {
            "version": { "type": "integer" },
            "definition_of_done": { "type": "string" },
            "deliverables": { "type": "array", "items": { "type": "string" } },
            "constraints": { "type": "array", "items": constraint },
            "acceptance_criteria": { "type": "array", "items": criterion }
        }
    })
}

pub fn plan_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["tasks"],
        "properties": {
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["tmp_id", "title", "role"],
                    "properties": {
                        "tmp_id": { "type": "string" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "role": { "type": "string" },
                        "objective_gate": { "type": "boolean" },
                        "contract": contract_schema()
                    }
                }
            },
            "edges": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["from_tmp", "to_tmp"],
                    "properties": {
                        "from_tmp": { "type": "string" },
                        "to_tmp": { "type": "string" }
                    }
                }
            },
            "reasoning": { "type": "string" }
        }
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationFault {
    NoTasks,
    TooManyTasks {
        got: usize,
        max: usize,
    },
    TooManyEdges {
        got: usize,
        max: usize,
    },
    DuplicateTmpId(String),
    UnknownTmpId(String),
    SelfEdge(String),
    EmptyTitle(String),
    TitleTooLong {
        tmp_id: String,
        len: usize,
        max: usize,
    },
    UnknownRole {
        tmp_id: String,
        role: String,
    },
    NoObjectiveGate,
    CycleInPlan,
}

impl std::fmt::Display for ValidationFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTasks => write!(f, "the plan contains no tasks"),
            Self::TooManyTasks { got, max } => {
                write!(f, "{got} tasks exceeds the limit of {max}")
            }
            Self::TooManyEdges { got, max } => {
                write!(f, "{got} edges exceeds the limit of {max}")
            }
            Self::DuplicateTmpId(id) => write!(f, "tmp_id {id:?} is used more than once"),
            Self::UnknownTmpId(id) => write!(f, "edge references unknown tmp_id {id:?}"),
            Self::SelfEdge(id) => write!(f, "task {id:?} depends on itself"),
            Self::EmptyTitle(id) => write!(f, "task {id:?} has an empty title"),
            Self::TitleTooLong { tmp_id, len, max } => {
                write!(f, "task {tmp_id:?} title is {len} chars, limit {max}")
            }
            Self::UnknownRole { tmp_id, role } => write!(
                f,
                "task {tmp_id:?} names role {role:?}, which is not an available agent role"
            ),
            Self::NoObjectiveGate => write!(
                f,
                "no task is marked objective_gate, so completion could never be determined"
            ),
            Self::CycleInPlan => write!(f, "the proposed dependencies contain a cycle"),
        }
    }
}

/// Feedback for the single repair round-trip. Phrased as instructions because it is pasted
/// straight into the retry prompt.
pub fn describe_faults(faults: &[ValidationFault]) -> String {
    let mut out =
        String::from("Your previous plan was rejected for these reasons. Fix all of them:\n");
    for fault in faults {
        out.push_str(&format!("- {fault}\n"));
    }
    out
}

/// Validates a proposed plan against limits and the roles that actually exist.
///
/// `available_roles` is computed by code from the configured team, which is what stops a planner
/// inventing a "Database Engineer" that does not exist and stranding the task unassignable.
pub fn validate_plan(
    plan: &ProposedPlan,
    available_roles: &[String],
    limits: PlanLimits,
) -> Vec<ValidationFault> {
    let mut faults = Vec::new();

    if plan.tasks.is_empty() {
        faults.push(ValidationFault::NoTasks);
        return faults;
    }
    if plan.tasks.len() > limits.max_tasks {
        faults.push(ValidationFault::TooManyTasks {
            got: plan.tasks.len(),
            max: limits.max_tasks,
        });
    }
    if plan.edges.len() > limits.max_edges {
        faults.push(ValidationFault::TooManyEdges {
            got: plan.edges.len(),
            max: limits.max_edges,
        });
    }

    let mut seen = std::collections::HashSet::new();
    for task in &plan.tasks {
        if !seen.insert(task.tmp_id.clone()) {
            faults.push(ValidationFault::DuplicateTmpId(task.tmp_id.clone()));
        }
        if task.title.trim().is_empty() {
            faults.push(ValidationFault::EmptyTitle(task.tmp_id.clone()));
        } else if task.title.chars().count() > limits.max_title_len {
            faults.push(ValidationFault::TitleTooLong {
                tmp_id: task.tmp_id.clone(),
                len: task.title.chars().count(),
                max: limits.max_title_len,
            });
        }
        if !available_roles.iter().any(|r| r == &task.role) {
            faults.push(ValidationFault::UnknownRole {
                tmp_id: task.tmp_id.clone(),
                role: task.role.clone(),
            });
        }
    }

    for edge in &plan.edges {
        if edge.from_tmp == edge.to_tmp {
            faults.push(ValidationFault::SelfEdge(edge.from_tmp.clone()));
        }
        for id in [&edge.from_tmp, &edge.to_tmp] {
            if !seen.contains(id) {
                faults.push(ValidationFault::UnknownTmpId(id.clone()));
            }
        }
    }

    // Without a gate, `objective_satisfied()` can never be true and the run would never finish.
    if !plan.tasks.iter().any(|t| t.objective_gate) {
        faults.push(ValidationFault::NoObjectiveGate);
    }

    if faults.is_empty() && plan_has_cycle(plan) {
        faults.push(ValidationFault::CycleInPlan);
    }

    faults
}

fn plan_has_cycle(plan: &ProposedPlan) -> bool {
    use std::collections::HashMap;

    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &plan.edges {
        adjacency
            .entry(edge.from_tmp.as_str())
            .or_default()
            .push(edge.to_tmp.as_str());
    }

    // 0 = unvisited, 1 = on the current path, 2 = finished.
    let mut marks: HashMap<&str, u8> = HashMap::new();
    let mut stack: Vec<(&str, usize)> = Vec::new();

    for task in &plan.tasks {
        let start = task.tmp_id.as_str();
        if marks.get(start).copied().unwrap_or(0) != 0 {
            continue;
        }
        stack.push((start, 0));
        marks.insert(start, 1);

        while let Some((node, index)) = stack.pop() {
            let neighbours = adjacency.get(node).cloned().unwrap_or_default();
            if index < neighbours.len() {
                stack.push((node, index + 1));
                let next = neighbours[index];
                match marks.get(next).copied().unwrap_or(0) {
                    1 => return true,
                    0 => {
                        marks.insert(next, 1);
                        stack.push((next, 0));
                    }
                    _ => {}
                }
            } else {
                marks.insert(node, 2);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// D2 — choose an assignee
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssignmentChoice {
    pub agent_id: String,
    #[serde(default)]
    pub reason: String,
}

pub fn assignment_schema(eligible: &[AgentId]) -> serde_json::Value {
    // The eligible set is an enum in the schema, so the CLI itself rejects an invented id before
    // we ever see it. Validation still re-checks: the schema is a convenience, not the guarantee.
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["agent_id"],
        "properties": {
            "agent_id": {
                "type": "string",
                "enum": eligible.iter().map(|a| a.to_string()).collect::<Vec<_>>()
            },
            "reason": { "type": "string" }
        }
    })
}

/// Resolves a choice against the eligible set, falling back deterministically.
///
/// The fallback is the point: assignment is never worth a repair round-trip, because any eligible
/// agent can do the work and stalling the loop to re-ask costs more than a suboptimal choice.
pub fn resolve_assignment(
    choice: Option<&AssignmentChoice>,
    eligible: &[AgentId],
    load: &dyn Fn(AgentId) -> usize,
) -> Option<(AgentId, AssignmentSource)> {
    if eligible.is_empty() {
        return None;
    }

    if let Some(choice) = choice {
        if let Some(chosen) = eligible.iter().find(|id| id.to_string() == choice.agent_id) {
            return Some((*chosen, AssignmentSource::Model));
        }
    }

    // Least-loaded, with the id as a tiebreak so the outcome is reproducible in a replay.
    let mut sorted: Vec<AgentId> = eligible.to_vec();
    sorted.sort_by_key(|id| (load(*id), id.to_string()));
    Some((sorted[0], AssignmentSource::Fallback))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentSource {
    Model,
    /// The model's answer was unusable or absent, so code chose. Recorded so the decision log
    /// shows when the model was not actually driving.
    Fallback,
}

// ---------------------------------------------------------------------------
// D3 — review verdict
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    PassWithNotes,
    Fail,
    /// The reviewer could not tell. Triggers one re-review with a fresh session, then escalation
    /// — never silently treated as either outcome.
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionJudgement {
    pub id: String,
    pub met: bool,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewResult {
    pub verdict: Verdict,
    #[serde(default)]
    pub criteria: Vec<CriterionJudgement>,
    #[serde(default)]
    pub blocking_findings: Vec<String>,
    #[serde(default)]
    pub summary: String,
}

pub fn review_schema(criterion_ids: &[String]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict", "summary"],
        "properties": {
            "verdict": {
                "type": "string",
                "enum": ["pass", "pass_with_notes", "fail", "inconclusive"]
            },
            "criteria": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "met"],
                    "properties": {
                        // Constrained to the contract's own ids so a verdict cannot judge a
                        // criterion that does not exist.
                        "id": { "type": "string", "enum": criterion_ids },
                        "met": { "type": "boolean" },
                        "evidence": { "type": "string" }
                    }
                }
            },
            "blocking_findings": { "type": "array", "items": { "type": "string" } },
            "summary": { "type": "string" }
        }
    })
}

/// Cross-checks a verdict against the contract and against what the gate already measured.
///
/// A reviewer claiming `Pass` while marking a criterion unmet is contradicting itself; trusting
/// the verdict over its own evidence is how false success gets through.
pub fn validate_review(
    review: &ReviewResult,
    criterion_ids: &[String],
) -> Result<Verdict, Vec<String>> {
    let mut faults = Vec::new();

    for judged in &review.criteria {
        if !criterion_ids.contains(&judged.id) {
            faults.push(format!(
                "verdict judges criterion {:?}, which is not in the contract",
                judged.id
            ));
        }
    }

    let any_unmet = review.criteria.iter().any(|c| !c.met);
    if matches!(review.verdict, Verdict::Pass | Verdict::PassWithNotes) && any_unmet {
        faults.push(
            "verdict is a pass while at least one criterion is marked unmet; the verdict \
             contradicts its own evidence"
                .into(),
        );
    }

    if review.verdict == Verdict::Fail && review.blocking_findings.is_empty() {
        faults.push(
            "verdict is fail but no blocking finding was given, so the developer would have \
             nothing to act on"
                .into(),
        );
    }

    if faults.is_empty() {
        Ok(review.verdict)
    } else {
        Err(faults)
    }
}

// ---------------------------------------------------------------------------
// D5 — failure disposition
// ---------------------------------------------------------------------------

/// Actions the failure handler may take. The legal set is computed by code from the task's actual
/// state; the model only ranks within it, so it cannot propose retrying a task with no attempts
/// left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureAction {
    RetrySameAgent,
    RestartSession,
    ReassignToDifferentAgent,
    Escalate,
    FailPermanently,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureDisposition {
    pub action: FailureAction,
    #[serde(default)]
    pub reason: String,
}

/// The actions genuinely available, given attempts, eligible agents and whether a human could
/// plausibly help.
pub fn legal_failure_actions(
    attempts_remaining: u32,
    other_agents_available: bool,
    is_objective_gate: bool,
) -> Vec<FailureAction> {
    let mut legal = Vec::new();

    if attempts_remaining > 0 {
        legal.push(FailureAction::RetrySameAgent);
        legal.push(FailureAction::RestartSession);
        if other_agents_available {
            legal.push(FailureAction::ReassignToDifferentAgent);
        }
    }

    // Always available: a human can nearly always unblock something.
    legal.push(FailureAction::Escalate);

    // Only offered when nothing else is left, and never for a gating task — failing one of those
    // autonomously would abandon the objective without asking.
    if attempts_remaining == 0 && !is_objective_gate {
        legal.push(FailureAction::FailPermanently);
    }

    legal
}

/// Applies the model's disposition if legal, otherwise the most conservative legal action.
///
/// Conservative means "prefer asking a human over giving up": escalation preserves work, and a
/// wrong autonomous failure destroys it.
pub fn resolve_failure_action(
    proposed: Option<FailureAction>,
    legal: &[FailureAction],
) -> (FailureAction, AssignmentSource) {
    if let Some(action) = proposed {
        if legal.contains(&action) {
            return (action, AssignmentSource::Model);
        }
    }

    let fallback = if legal.contains(&FailureAction::Escalate) {
        FailureAction::Escalate
    } else {
        legal
            .first()
            .copied()
            .unwrap_or(FailureAction::FailPermanently)
    };
    (fallback, AssignmentSource::Fallback)
}

// ---------------------------------------------------------------------------
// Decision log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    Code,
    Claude,
    Human,
}

/// One recorded decision. Enough to replay the run: the inputs are captured, and the pure stages
/// are functions of (snapshot, decision).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub iteration: u32,
    pub stage: String,
    pub kind: String,
    pub decided_by: DecidedBy,
    #[serde(default)]
    pub rule_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<TaskId>,
    #[serde(default)]
    pub inputs_digest: Option<String>,
    #[serde(default)]
    pub validation_errors: Vec<String>,
    pub repair_count: u32,
    #[serde(default)]
    pub rationale: String,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}
