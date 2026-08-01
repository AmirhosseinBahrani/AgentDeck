//! Runs the stage pipeline.
//!
//! The stages themselves stay pure where they can: `Plan` and `Assign` consult a model, but the
//! result only reaches the graph through the validation ladder, and every state change goes
//! through `deck_core`'s transition table. The driver's own job is sequencing and I/O.

use crate::autonomy::{ApprovalQueue, Autonomy, NoApprovals};
use crate::contract::{run_gate, validate_and_repair, GateOutcome, TaskContract};
use crate::decision::*;
use crate::escalation::{AnswerQueue, Escalation, EscalationAnswer, EscalationKind, NoAnswers};
use crate::graph::{Edge, EdgeKind, Mutation, TaskGraph};
use crate::guidance::{Guidance, GuidanceQueue, NoGuidance};
use crate::loop_engine::*;
use crate::planner::{Decisions, Planner};
use crate::workspaces::{render_brief, DispatchRequest, NoReports, ReportQueue, Workspaces};
use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::domain::task::{apply, TaskEvent, TaskState, TaskStatus};
use deck_core::git::{Contribution, IntegrationOutcome};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

/// A role the supervisor may assign work to.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamMember {
    pub agent_id: AgentId,
    pub role: String,
}

/// Everything the driver needs that it does not own.
pub struct RunConfig {
    pub objective: String,
    pub team: Vec<TeamMember>,
    /// Injected into any contract that arrives without executable verification.
    pub default_test_command: String,
    /// Fallback verification directory, used only for a task with no worktree — which should not
    /// happen once dispatch has run, and is treated as inconclusive rather than failed.
    pub verification_root: PathBuf,
    pub limits: RunLimits,
    pub plan_limits: PlanLimits,
    pub per_call_budget_usd: f64,
    pub verification_timeout: Duration,
    /// How much the supervisor may do without being asked. Enforced at dispatch, which is where
    /// a real process gets a real worktree — anywhere earlier and the mode would be advisory.
    pub autonomy: Autonomy,
}

impl RunConfig {
    pub fn roles(&self) -> Vec<String> {
        let mut roles: Vec<String> = self.team.iter().map(|m| m.role.clone()).collect();
        roles.sort();
        roles.dedup();
        roles
    }

    pub fn agents_with_role(&self, role: &str) -> Vec<AgentId> {
        self.team
            .iter()
            .filter(|m| m.role == role)
            .map(|m| m.agent_id)
            .collect()
    }
}

/// Live run state the driver mutates.
pub struct Run {
    pub state: RunState,
    pub graph: TaskGraph,
    pub log: IterationLog,
    pub receipts: StageReceipts,
    /// Contract and role per task, kept beside the graph because the graph stores lifecycle only.
    pub contracts: HashMap<TaskId, TaskContract>,
    pub roles: HashMap<TaskId, String>,
    pub titles: HashMap<TaskId, String>,
    /// Live session per dispatched task, so the app can attach a transcript view or kill it.
    pub sessions: HashMap<TaskId, deck_core::domain::ids::SessionId>,
    /// The branch each task's work is on, recorded at dispatch. The roster shows it, and the
    /// registry that knows it is not reachable from a snapshot.
    pub branches: HashMap<TaskId, String>,
    /// Tasks whose agent has claimed completion and are awaiting verification.
    pub awaiting_verification: Vec<TaskId>,
    /// Dispatches a human has approved. Consumed on use, so approving once starts one agent.
    pub approved: HashSet<TaskId>,
    /// Tasks that would have started but are waiting on a human. Surfaced so the operator can
    /// see what their attention is holding up rather than watching an apparently idle run.
    pub awaiting_approval: Vec<TaskId>,
    /// Questions the run needs answered. Records rather than a counter, because a number tells
    /// the operator that they are needed without telling them what for.
    pub escalations: Vec<Escalation>,
    /// Standing instructions from the operator, folded into planning and assignment prompts.
    pub guidance: Vec<Guidance>,
}

impl Run {
    pub fn new() -> Self {
        Self {
            state: RunState::default(),
            graph: TaskGraph::new(),
            log: IterationLog::default(),
            receipts: StageReceipts::new(),
            contracts: HashMap::new(),
            roles: HashMap::new(),
            titles: HashMap::new(),
            sessions: HashMap::new(),
            branches: HashMap::new(),
            awaiting_verification: Vec::new(),
            approved: HashSet::new(),
            awaiting_approval: Vec::new(),
            escalations: Vec::new(),
            guidance: Vec::new(),
        }
    }

    /// Raises a question for the operator and parks the run on it.
    ///
    /// Deduplicated by (kind, task): a sweep runs every couple of seconds, and without this the
    /// same unanswered question would pile up until the inbox was unreadable.
    pub fn escalate(
        &mut self,
        kind: EscalationKind,
        task_id: Option<TaskId>,
        question: impl Into<String>,
        detail: impl Into<String>,
    ) {
        if self
            .escalations
            .iter()
            .any(|e| e.kind == kind && e.task_id == task_id)
        {
            return;
        }
        self.escalations.push(Escalation::new(
            kind,
            task_id,
            question,
            detail,
            self.state.iteration,
        ));
        self.state.open_escalations = self.escalations.len();
        self.state.phase = RunPhase::BlockedOnHuman;
    }

    /// Applies a typed answer and unparks the run if nothing else is outstanding.
    ///
    /// Returns false for an id that is not open, which is the ordinary result of a double click
    /// or a stale window — answering the same question twice must not apply it twice.
    pub fn answer(&mut self, escalation_id: &str, answer: EscalationAnswer) -> bool {
        let Some(index) = self.escalations.iter().position(|e| e.id == escalation_id) else {
            return false;
        };
        let escalation = self.escalations.remove(index);
        self.state.open_escalations = self.escalations.len();

        match answer {
            EscalationAnswer::RetryPlanning => {
                // Clearing the graph is what makes the Plan stage run again; it guards on empty.
                self.graph = TaskGraph::new();
                self.contracts.clear();
                self.roles.clear();
                self.titles.clear();
            }
            EscalationAnswer::RetryTask { task_id } => {
                if let Some(current) = self.graph.get(task_id).cloned() {
                    let mut next = current.clone();
                    // Refund the attempt the failure consumed, otherwise "try again" would be
                    // refused immediately by the very cap that raised this question.
                    next.attempts = next.attempts.saturating_sub(1);
                    next.status = TaskStatus::Queued;
                    next.failure_reason = None;
                    self.graph.set_state(next);
                }
            }
            EscalationAnswer::AbandonTask { task_id } => {
                if let Some(current) = self.graph.get(task_id).cloned() {
                    if let Ok(next) = apply(&current, TaskEvent::Cancelled) {
                        self.graph.set_state(next);
                    }
                }
            }
            EscalationAnswer::Reintegrate => {
                self.state.integrated = false;
            }
            EscalationAnswer::CancelRun => {
                self.state.phase = RunPhase::Cancelled;
                return true;
            }
        }

        self.log.record_human_decision(
            self.state.iteration,
            Stage::Escalate,
            "escalation_answered",
            &format!(
                "{:?} answered for {:?}",
                escalation.kind, escalation.task_id
            ),
        );

        // Only resume once nothing else is outstanding; unparking with questions still open
        // would let the run carry on past a decision the operator has not made.
        if self.escalations.is_empty() {
            self.state.phase = RunPhase::Monitoring;
        }
        true
    }

    fn load_of(&self, agent: AgentId) -> usize {
        self.graph
            .tasks()
            .filter(|t| t.assignee == Some(agent) && t.status.is_active())
            .count()
    }
}

impl Default for Run {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum IterationOutcome {
    /// The pipeline ran. `dispatched` are tasks now ready for an agent to be spawned.
    Advanced {
        dispatched: Vec<TaskId>,
    },
    /// The sweep decided nothing was worth doing.
    Idle,
    Terminal(RunPhase),
}

pub struct Driver<'a> {
    pub config: &'a RunConfig,
    pub planner: &'a dyn Planner,
    pub workspaces: &'a dyn Workspaces,
    pub reports: &'a dyn ReportQueue,
    pub approvals: &'a dyn ApprovalQueue,
    pub answers: &'a dyn AnswerQueue,
    pub guidance: &'a dyn GuidanceQueue,
}

impl<'a> Driver<'a> {
    pub fn new(
        config: &'a RunConfig,
        planner: &'a dyn Planner,
        workspaces: &'a dyn Workspaces,
    ) -> Self {
        Self {
            config,
            planner,
            workspaces,
            reports: &NoReports,
            approvals: &NoApprovals,
            answers: &NoAnswers,
            guidance: &NoGuidance,
        }
    }

    /// Supplies live worker reports. Without one the driver runs against no agent input, which is
    /// what the scripted tests want.
    pub fn with_reports(mut self, reports: &'a dyn ReportQueue) -> Self {
        self.reports = reports;
        self
    }

    /// Supplies dispatch approvals from a human. Only consulted in modes that require them.
    pub fn with_approvals(mut self, approvals: &'a dyn ApprovalQueue) -> Self {
        self.approvals = approvals;
        self
    }

    /// Supplies answers to open escalations.
    pub fn with_answers(mut self, answers: &'a dyn AnswerQueue) -> Self {
        self.answers = answers;
        self
    }

    /// Supplies standing instructions from the operator.
    pub fn with_guidance(mut self, guidance: &'a dyn GuidanceQueue) -> Self {
        self.guidance = guidance;
        self
    }

    /// Runs one iteration if the sweep says it is warranted.
    pub async fn step(&self, run: &mut Run, dirty: bool) -> IterationOutcome {
        // Before the sweep, not after. An answer that arrived while the run was parked has to be
        // visible to the very check that decides whether it is still parked — applying it later
        // would leave the run blocked for one more cycle on a question already answered.
        for (id, answer) in self.answers.drain() {
            run.answer(&id, answer);
        }

        // Applied before the sweep, like answers: guidance that asks for a replan has to be
        // visible to the check that decides whether there is anything to do this iteration.
        for note in self.guidance.drain() {
            run.log.record_human_decision(
                run.state.iteration,
                Stage::Plan,
                "operator_guidance",
                &note.text,
            );
            if note.replan {
                // Clearing the graph is what makes the Plan stage run again — it guards on
                // empty. Contracts and titles go with it so nothing survives that described a
                // task the new plan may not contain.
                run.graph = TaskGraph::new();
                run.contracts.clear();
                run.roles.clear();
                run.titles.clear();
                run.state.integrated = false;
            }
            run.guidance.push(note);
        }

        let outcome = sweep(&run.state, &run.graph, self.config.limits, dirty);

        for note in &outcome.notes {
            run.log.record_code_decision(
                run.state.iteration,
                Stage::Observe,
                "sweep",
                "sweep",
                note,
            );
        }

        if let Some(phase) = outcome.terminal {
            run.state.phase = phase;
            return IterationOutcome::Terminal(phase);
        }
        if let Some(phase) = outcome.phase {
            run.state.phase = phase;
        }
        if !outcome.should_iterate {
            return IterationOutcome::Idle;
        }

        let iteration = run.state.iteration;
        let mut dispatched = Vec::new();

        for stage in run.receipts.remaining(iteration) {
            // A stage that escalated or ended the run stops the pipeline. Continuing would let a
            // later stage overwrite the phase and hide the fact that a human is needed.
            if run.state.phase.is_terminal() || run.state.phase == RunPhase::BlockedOnHuman {
                break;
            }

            match stage {
                Stage::IngestReports => self.stage_ingest_reports(run),
                Stage::Reap => self.stage_reap(run),
                Stage::Plan if run.graph.is_empty() => self.stage_plan(run).await,
                Stage::Assign => self.stage_assign(run).await,
                Stage::Dispatch => dispatched = self.stage_dispatch(run).await,
                Stage::Verify => self.stage_verify(run).await,
                Stage::Judge => self.stage_judge(run).await,
                Stage::CompletionCheck => self.stage_completion_check(run).await,
                Stage::Commit => {
                    run.state.iteration += 1;
                    run.state.spent_usd = run.log.cost();
                }
                _ => {}
            }
            run.receipts.record(iteration, stage);
        }

        IterationOutcome::Advanced { dispatched }
    }

    // -----------------------------------------------------------------------
    // Reap — pure: notices agents that died without saying anything
    // -----------------------------------------------------------------------

    /// Fails tasks whose agent is gone but never claimed completion.
    ///
    /// `claim_task_done` is the only legal completion path, so a session that simply exits has
    /// failed — it has not quietly succeeded. Without this the task sits in `Running` forever:
    /// nothing will report on it, no trigger will arrive, and the run stalls with no indication
    /// of why. A crashed CLI, an out-of-memory kill or an agent that talked itself into calling
    /// it a day all look identical from here, and all of them mean the work did not finish.
    ///
    /// The attempt was already counted when the agent started, so this only decides whether any
    /// remain — a retry re-dispatches, and an exhausted task fails for good.
    fn stage_reap(&self, run: &mut Run) {
        let dead: Vec<TaskId> = run
            .graph
            .tasks()
            .filter(|t| t.status == TaskStatus::Running)
            // Only tasks we actually dispatched. `None` means no agent was ever started, which
            // is a scheduling state rather than a death.
            .filter(|t| self.workspaces.agent_alive(t.id) == Some(false))
            .map(|t| t.id)
            .collect();

        for id in dead {
            let Some(current) = run.graph.get(id).cloned() else {
                continue;
            };
            let attempts_left = current.attempts < current.limits.max_attempts;

            if let Ok(next) = apply(
                &current,
                TaskEvent::Failed {
                    reason: "the agent exited without claiming the task done".into(),
                },
            ) {
                run.graph.set_state(next);
            }
            // Nothing left to try is a decision only a human can take further, so it is raised
            // rather than left as a quietly failed task nobody is told about.
            if !attempts_left {
                let title = run
                    .titles
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| id.to_string());
                run.escalate(
                    EscalationKind::AttemptsExhausted,
                    Some(id),
                    format!("\u{201c}{title}\u{201d} has run out of attempts"),
                    "Its agent exited without claiming the task done, every time.".to_string(),
                );
            }

            run.log.record_code_decision(
                run.state.iteration,
                Stage::Reap,
                "agent_died",
                if attempts_left {
                    "retrying"
                } else {
                    "attempts_exhausted"
                },
                &format!("task {id} lost its agent without a completion claim"),
            );
        }
    }

    // -----------------------------------------------------------------------
    // Completion check — the integration gate. Pure code; no model is consulted
    // -----------------------------------------------------------------------

    /// Merges every completed task's branch and runs the project's tests against the result.
    ///
    /// Runs only once every objective-gating task is complete, because merging half a plan
    /// proves nothing and burns a test run each iteration.
    ///
    /// This exists because "every task passed" means every task passed *alone*. Agents work in
    /// separate worktrees on separate branches, so two green tasks can still be incompatible —
    /// one renames what the other calls. Nothing in per-task verification can see that, which
    /// makes this the only check standing between a green dashboard and a pile of branches that
    /// do not combine.
    async fn stage_completion_check(&self, run: &mut Run) {
        if run.state.integrated || !run.graph.objective_satisfied() {
            return;
        }

        // Deterministic order, so the same run integrates the same way twice and a conflict is
        // reproducible rather than a function of hash iteration order.
        let mut contributions: Vec<Contribution> = run
            .graph
            .tasks()
            .filter(|t| t.status == TaskStatus::Completed)
            .filter_map(|t| {
                self.workspaces.branch(t.id).map(|branch| Contribution {
                    task_id: t.id.to_string(),
                    branch,
                })
            })
            .collect();
        contributions.sort_by(|a, b| a.task_id.cmp(&b.task_id));

        let outcome = self
            .workspaces
            .integrate(
                &contributions,
                &self.config.default_test_command,
                self.config.verification_timeout,
            )
            .await;

        match outcome {
            IntegrationOutcome::Integrated { merged } => {
                run.state.integrated = true;
                run.log.record_code_decision(
                    run.state.iteration,
                    Stage::CompletionCheck,
                    "integration",
                    "merged_and_green",
                    &format!(
                        "{} branches merged and the project tests passed",
                        merged.len()
                    ),
                );
            }

            // A conflict means two agents were given overlapping work. Resolving it for them
            // would mean choosing which agent's work to discard, which is not a decision code
            // or a model should be making silently.
            IntegrationOutcome::Conflicted {
                branch,
                task_id,
                files,
            } => {
                run.escalate(
                    EscalationKind::IntegrationConflict,
                    None,
                    "Two agents produced work that will not merge",
                    format!(
                        "{branch} conflicts with work already merged, in: {}",
                        if files.is_empty() {
                            "unknown files".to_string()
                        } else {
                            files.join(", ")
                        }
                    ),
                );
                run.log.record_code_decision(
                    run.state.iteration,
                    Stage::CompletionCheck,
                    "integration_conflict",
                    "overlapping_work",
                    &format!(
                        "{branch} (task {task_id}) conflicts with work already merged, in: {}",
                        if files.is_empty() {
                            "unknown files".to_string()
                        } else {
                            files.join(", ")
                        }
                    ),
                );
            }

            // Every task passed and the combination does not work. No individual task is at
            // fault, so blaming one by reopening it would send an agent to fix code that is
            // correct on its own.
            IntegrationOutcome::TestsFailed { output } => {
                run.escalate(
                    EscalationKind::IntegrationBroken,
                    None,
                    "Every task passed, but the branches do not work together",
                    output.clone(),
                );
                run.log.record_code_decision(
                    run.state.iteration,
                    Stage::CompletionCheck,
                    "integration_tests_failed",
                    "combined_result_broken",
                    &output,
                );
            }

            IntegrationOutcome::Inconclusive { reason } => {
                run.escalate(
                    EscalationKind::IntegrationBroken,
                    None,
                    "The branches could not be merged and tested",
                    reason.clone(),
                );
                run.log.record_code_decision(
                    run.state.iteration,
                    Stage::CompletionCheck,
                    "integration_inconclusive",
                    "environment",
                    &reason,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Ingest reports — pure: applies what agents said, decides nothing
    // -----------------------------------------------------------------------

    fn stage_ingest_reports(&self, run: &mut Run) {
        use deck_core::reporting::WorkerReport;

        for (task_id, report) in self.reports.drain() {
            match report {
                // Queues the task for verification. Note this does not complete it: the gate runs
                // next, and a claim whose criteria fail comes straight back as a rejection.
                WorkerReport::ClaimTaskDone { summary } => {
                    if claim_done(run, task_id) {
                        run.log.record_code_decision(
                            run.state.iteration,
                            Stage::IngestReports,
                            "claim_task_done",
                            "worker_claim",
                            &summary,
                        );
                    } else {
                        // A claim from a task that is not running is a protocol violation, not a
                        // completion — most likely a duplicate, or an agent outliving its task.
                        run.log.record_code_decision(
                            run.state.iteration,
                            Stage::IngestReports,
                            "claim_rejected",
                            "illegal_state",
                            &format!("task {task_id} claimed completion from an illegal state"),
                        );
                    }
                }

                WorkerReport::RaiseBlocker { reason } => {
                    if let Some(current) = run.graph.get(task_id).cloned() {
                        if let Ok(next) = apply(
                            &current,
                            TaskEvent::Blocked {
                                reason: reason.clone(),
                            },
                        ) {
                            run.graph.set_state(next);
                        }
                    }
                    let title = run
                        .titles
                        .get(&task_id)
                        .cloned()
                        .unwrap_or_else(|| task_id.to_string());
                    run.escalate(
                        EscalationKind::TaskBlocked,
                        Some(task_id),
                        format!("\u{201c}{title}\u{201d} is blocked"),
                        reason.clone(),
                    );
                    run.log.record_code_decision(
                        run.state.iteration,
                        Stage::IngestReports,
                        "raise_blocker",
                        "worker_blocked",
                        &reason,
                    );
                }

                // Advisory only. The supervisor also derives telemetry the worker cannot
                // influence, so progress is recorded rather than acted upon.
                WorkerReport::ReportProgress { summary, .. } => {
                    run.log.record_code_decision(
                        run.state.iteration,
                        Stage::IngestReports,
                        "report_progress",
                        "worker_progress",
                        &summary,
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Plan
    // -----------------------------------------------------------------------

    async fn stage_plan(&self, run: &mut Run) {
        let roles = self.config.roles();
        let decisions = Decisions::new(self.planner);

        let prompt = self.plan_prompt(&roles, &run.guidance, None);
        let first = decisions
            .plan(prompt, plan_schema(), self.config.per_call_budget_usd)
            .await;

        // Kept so the escalation can say what actually went wrong. A planner that could not be
        // reached and a model that answered badly need completely different fixes, and reporting
        // both as "failed validation" sends the operator looking in the wrong place.
        let mut unreachable: Option<String> = None;

        let (plan, cost, faults, repairs) = match first {
            Ok((plan, cost)) => {
                let faults = validate_plan(&plan, &roles, self.config.plan_limits);
                if faults.is_empty() {
                    (Some(plan), cost, Vec::new(), 0)
                } else {
                    // Exactly one repair round-trip. Looping here is how a bad prompt would
                    // silently consume the whole budget.
                    let retry_prompt =
                        self.plan_prompt(&roles, &run.guidance, Some(&describe_faults(&faults)));
                    match decisions
                        .plan(retry_prompt, plan_schema(), self.config.per_call_budget_usd)
                        .await
                    {
                        Ok((retry, retry_cost)) => {
                            let retry_faults =
                                validate_plan(&retry, &roles, self.config.plan_limits);
                            let total = cost.unwrap_or(0.0) + retry_cost.unwrap_or(0.0);
                            if retry_faults.is_empty() {
                                (Some(retry), Some(total), faults, 1)
                            } else {
                                (None, Some(total), retry_faults, 1)
                            }
                        }
                        Err(e) => {
                            unreachable = Some(e.to_string());
                            (None, cost, faults, 1)
                        }
                    }
                }
            }
            Err(e) => {
                unreachable = Some(e.to_string());
                (None, None, Vec::new(), 0)
            }
        };

        let Some(plan) = plan else {
            // No deterministic fallback exists for planning — code cannot invent a decomposition.
            // Escalating is the honest outcome.
            run.escalate(
                EscalationKind::PlanningFailed,
                None,
                "The objective could not be turned into a plan",
                unreachable
                    .clone()
                    .unwrap_or_else(|| describe_faults(&faults)),
            );
            let mut errors: Vec<String> = faults.iter().map(|f| f.to_string()).collect();
            let rationale = match &unreachable {
                Some(detail) => {
                    errors.push(detail.clone());
                    "the planner could not be reached or gave an unusable answer; escalating"
                }
                None => "planning failed validation twice; escalating rather than guessing",
            };
            run.log.record_model_decision(
                run.state.iteration,
                Stage::Plan,
                "decompose_objective",
                rationale,
                errors,
                repairs,
                cost,
            );
            return;
        };

        // Real ids are minted here, after validation — a model never supplies one, so it cannot
        // address an existing task by guessing.
        let mut ids: HashMap<String, TaskId> = HashMap::new();
        let mut tasks = Vec::new();

        for proposed in &plan.tasks {
            let id = TaskId::new();
            ids.insert(proposed.tmp_id.clone(), id);

            let mut state = TaskState::new(id);
            state.objective_gate = proposed.objective_gate;
            tasks.push(state);

            let mut contract = proposed.contract.clone();
            let contract_repairs =
                validate_and_repair(&mut contract, &self.config.default_test_command);
            for repair in &contract_repairs {
                run.log.record_code_decision(
                    run.state.iteration,
                    Stage::Plan,
                    "contract_repair",
                    "contract_must_be_verifiable",
                    &format!("{repair:?}"),
                );
            }
            run.contracts.insert(id, contract);
            run.roles.insert(id, proposed.role.clone());
            run.titles.insert(id, proposed.title.clone());
        }

        let edges = plan
            .edges
            .iter()
            .filter_map(|e| {
                Some(Edge {
                    from: *ids.get(&e.from_tmp)?,
                    to: *ids.get(&e.to_tmp)?,
                    kind: EdgeKind::Hard,
                })
            })
            .collect();

        // The graph re-validates independently. Belt and braces: the plan validator and the graph
        // disagreeing would be a bug, and this is where it would surface rather than corrupt state.
        if let Err(e) = run.graph.apply(Mutation {
            add_tasks: tasks,
            add_edges: edges,
        }) {
            run.state.phase = RunPhase::BlockedOnHuman;
            run.log.record_code_decision(
                run.state.iteration,
                Stage::Plan,
                "graph_rejected_plan",
                "graph_invariants",
                &e.to_string(),
            );
            return;
        }

        run.log.record_model_decision(
            run.state.iteration,
            Stage::Plan,
            "decompose_objective",
            &plan.reasoning,
            faults.iter().map(|f| f.to_string()).collect(),
            repairs,
            cost,
        );
        run.state.phase = RunPhase::Dispatching;
    }

    fn plan_prompt(&self, roles: &[String], notes: &[Guidance], repair: Option<&str>) -> String {
        let mut prompt = format!(
            "Decompose this objective into tasks for a small engineering team.\n\n\
             Objective: {}\n\n\
             Available roles: {}\n\n\
             Rules:\n\
             - Mark every task that the objective cannot be considered complete without as \
               objective_gate.\n\
             - Give each task acceptance criteria that can be checked by running a command.\n\
             - Use tmp_id values to express dependencies; they must not form a cycle.\n",
            self.config.objective,
            roles.join(", "),
        );
        if let Some(notes) = crate::guidance::render(notes) {
            prompt.push('\n');
            prompt.push_str(&notes);
        }
        if let Some(feedback) = repair {
            prompt.push('\n');
            prompt.push_str(feedback);
        }
        prompt
    }

    // -----------------------------------------------------------------------
    // Assign
    // -----------------------------------------------------------------------

    async fn stage_assign(&self, run: &mut Run) {
        let decisions = Decisions::new(self.planner);

        for task_id in run.graph.ready() {
            let Some(role) = run.roles.get(&task_id).cloned() else {
                continue;
            };
            let eligible = self.config.agents_with_role(&role);
            if eligible.is_empty() {
                continue;
            }

            // Only worth a model call when there is an actual choice to make.
            let had_a_choice = eligible.len() > 1;
            let choice = if had_a_choice {
                let prompt = format!(
                    "Choose which agent should take this task.\n\nTask: {}\nRole: {role}\n",
                    run.graph
                        .get(task_id)
                        .map(|t| t.id.to_string())
                        .unwrap_or_default()
                );
                decisions
                    .assign(
                        prompt,
                        assignment_schema(&eligible),
                        self.config.per_call_budget_usd,
                    )
                    .await
                    .ok()
            } else {
                None
            };

            let load = |id: AgentId| run.load_of(id);
            let Some((agent_id, source)) =
                resolve_assignment(choice.as_ref().map(|(c, _)| c), &eligible, &load)
            else {
                continue;
            };

            let Some(current) = run.graph.get(task_id).cloned() else {
                continue;
            };
            let staged = if current.status == TaskStatus::Backlog {
                apply(&current, TaskEvent::Enqueued).unwrap_or(current)
            } else {
                current
            };

            if let Ok(next) = apply(&staged, TaskEvent::Assigned { agent_id }) {
                run.graph.set_state(next);
                match source {
                    AssignmentSource::Model => run.log.record_model_decision(
                        run.state.iteration,
                        Stage::Assign,
                        "choose_assignee",
                        &choice.map(|(c, _)| c.reason).unwrap_or_default(),
                        Vec::new(),
                        0,
                        choice_cost(&eligible),
                    ),
                    // Two different things reach this arm and the log used to call both of
                    // them a failed model choice. With one candidate no model is consulted at
                    // all, and reporting that as "no usable model choice" sends whoever reads
                    // the log looking for a model problem that never happened.
                    AssignmentSource::Fallback if !had_a_choice => run.log.record_code_decision(
                        run.state.iteration,
                        Stage::Assign,
                        "choose_assignee",
                        "only_candidate",
                        &format!("{role} is the only agent for this task; assigned directly"),
                    ),
                    AssignmentSource::Fallback => run.log.record_code_decision(
                        run.state.iteration,
                        Stage::Assign,
                        "choose_assignee",
                        "least_loaded",
                        "the model's choice was unusable; assigned the least-loaded eligible agent",
                    ),
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Dispatch — pure: reports what should be started, does not spawn
    // -----------------------------------------------------------------------

    async fn stage_dispatch(&self, run: &mut Run) -> Vec<TaskId> {
        // Approvals granted since the last iteration. Drained here rather than applied on arrival
        // for the same reason worker reports are: a click lands whenever it lands.
        for id in self.approvals.drain() {
            run.approved.insert(id);
        }

        let ready: Vec<TaskId> = run
            .graph
            .tasks()
            .filter(|t| deck_core::domain::task::is_dispatchable(t))
            .map(|t| t.id)
            .collect();

        run.awaiting_approval.clear();
        let mut started = Vec::new();
        for id in ready {
            let Some(current) = run.graph.get(id).cloned() else {
                continue;
            };
            let Some(agent_id) = current.assignee else {
                continue;
            };

            // The enforcement point for autonomy, and the only one. Everything upstream is
            // reasoning; this is where a process gets spawned into a worktree with edit rights,
            // so a mode that did not stop it here would not be stopping anything.
            //
            // `remove` rather than `contains`: an approval authorises one start. Leaving it in
            // place would silently re-authorise every future retry of the same task.
            if self.config.autonomy.dispatch_needs_approval() && !run.approved.remove(&id) {
                run.awaiting_approval.push(id);
                continue;
            }
            let contract = run.contracts.get(&id).cloned().unwrap_or_default();
            let role = run.roles.get(&id).cloned().unwrap_or_default();
            let title = run
                .titles
                .get(&id)
                .cloned()
                .unwrap_or_else(|| id.to_string());

            let request = DispatchRequest {
                task_id: id,
                agent_id,
                agent_role: role,
                task_title: title.clone(),
                brief: render_brief(&title, &contract),
                contract,
            };

            match self.workspaces.dispatch(request).await {
                Ok(agent) => {
                    // Only move the task to Running once an agent is genuinely started. Marking it
                    // Running on intent would consume an attempt for work that never began.
                    if let Ok(next) = apply(&current, TaskEvent::Started) {
                        run.graph.set_state(next);
                        run.sessions.insert(id, agent.session_id);
                        if let Some(branch) = self.workspaces.branch(id) {
                            run.branches.insert(id, branch);
                        }
                        started.push(id);
                        run.log.record_code_decision(
                            run.state.iteration,
                            Stage::Dispatch,
                            "dispatch",
                            "assigned_and_ready",
                            &format!("task {id} started in {}", agent.worktree.display()),
                        );
                    }
                }
                Err(e) => {
                    // A worktree or spawn failure is an environment problem, not the agent's
                    // fault, so it escalates rather than burning the task's retry budget.
                    run.escalate(
                        EscalationKind::DispatchFailed,
                        Some(id),
                        format!("Could not start an agent for \u{201c}{title}\u{201d}"),
                        e.to_string(),
                    );
                    run.log.record_code_decision(
                        run.state.iteration,
                        Stage::Dispatch,
                        "dispatch_failed",
                        "environment",
                        &e.to_string(),
                    );
                    break;
                }
            }
        }

        if !started.is_empty() && run.state.phase != RunPhase::BlockedOnHuman {
            run.state.phase = RunPhase::Monitoring;
        }
        started
    }

    // -----------------------------------------------------------------------
    // Verify — pure code, runs the contract's commands itself
    // -----------------------------------------------------------------------

    async fn stage_verify(&self, run: &mut Run) {
        let pending = std::mem::take(&mut run.awaiting_verification);

        for task_id in pending {
            let Some(contract) = run.contracts.get(&task_id).cloned() else {
                continue;
            };

            // Each task is verified in the tree its own agent worked in. Verifying in a shared
            // directory would test the wrong code — and would most likely pass, which is the
            // dangerous direction.
            let worktree = self
                .workspaces
                .worktree(task_id)
                .unwrap_or_else(|| self.config.verification_root.clone());

            let outcome = run_gate(&worktree, &contract, self.config.verification_timeout).await;

            match outcome {
                GateOutcome::Failed { outcomes } => {
                    // The reviewer is never invoked. A model cannot argue past a red test if it
                    // never sees the work.
                    let failed: Vec<String> = outcomes
                        .iter()
                        .filter(|o| !o.passed)
                        .map(|o| format!("{}: {}", o.criterion_id, o.detail))
                        .collect();

                    let reason = failed.join("; ");
                    // A review failure normally sends the task round again. Manual mode does not
                    // get to do that: its whole claim is that nothing happens twice without a
                    // human seeing it happen once, and a silent retry is exactly that.
                    let event = if self.config.autonomy.may_retry() {
                        TaskEvent::ReviewFailed {
                            reason: reason.clone(),
                        }
                    } else {
                        let title = run
                            .titles
                            .get(&task_id)
                            .cloned()
                            .unwrap_or_else(|| task_id.to_string());
                        run.escalate(
                            EscalationKind::TaskBlocked,
                            Some(task_id),
                            format!("\u{201c}{title}\u{201d} failed its checks"),
                            reason.clone(),
                        );
                        TaskEvent::Blocked {
                            reason: reason.clone(),
                        }
                    };

                    if let Some(current) = run.graph.get(task_id).cloned() {
                        if let Ok(next) = apply(&current, event) {
                            run.graph.set_state(next);
                        }
                    }
                    run.log.record_code_decision(
                        run.state.iteration,
                        Stage::Verify,
                        "verification_gate",
                        "executable_criteria_failed",
                        &reason,
                    );
                }
                GateOutcome::Passed {
                    judgment_pending, ..
                } => {
                    run.log.record_code_decision(
                        run.state.iteration,
                        Stage::Verify,
                        "verification_gate",
                        "executable_criteria_passed",
                        &format!("{judgment_pending} criteria remain for the reviewer"),
                    );
                    if judgment_pending == 0 {
                        // Nothing left to judge, so no reviewer call is warranted.
                        if let Some(current) = run.graph.get(task_id).cloned() {
                            if let Ok(next) = apply(&current, TaskEvent::ReviewPassed) {
                                run.graph.set_state(next);
                            }
                        }
                    } else {
                        run.awaiting_verification.push(task_id);
                    }
                }
                GateOutcome::Inconclusive { reason } => {
                    // Not a review round: the work was never judged.
                    let title = run
                        .titles
                        .get(&task_id)
                        .cloned()
                        .unwrap_or_else(|| task_id.to_string());
                    run.escalate(
                        EscalationKind::TaskBlocked,
                        Some(task_id),
                        format!("\u{201c}{title}\u{201d} could not be checked"),
                        reason.clone(),
                    );
                    run.log.record_code_decision(
                        run.state.iteration,
                        Stage::Verify,
                        "verification_gate",
                        "inconclusive",
                        &reason,
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Judge — only reached for work whose executable criteria already passed
    // -----------------------------------------------------------------------

    async fn stage_judge(&self, run: &mut Run) {
        let pending = std::mem::take(&mut run.awaiting_verification);
        let decisions = Decisions::new(self.planner);
        let mut judged = 0usize;

        for task_id in pending {
            let Some(contract) = run.contracts.get(&task_id).cloned() else {
                continue;
            };
            let criterion_ids: Vec<String> = contract
                .acceptance_criteria
                .iter()
                .map(|c| c.id.clone())
                .collect();

            let prompt = format!(
                "Review this work against its contract.\n\nDefinition of done: {}\n\n\
                 Every executable criterion has already been verified by running it; you are \
                 judging only the criteria that require judgement.\n",
                contract.definition_of_done
            );

            let response = decisions
                .review(
                    prompt,
                    review_schema(&criterion_ids),
                    self.config.per_call_budget_usd,
                )
                .await;

            let (event, errors, cost) = match response {
                Ok((review, cost)) => match validate_review(&review, &criterion_ids) {
                    Ok(Verdict::Pass) | Ok(Verdict::PassWithNotes) => {
                        (Some(TaskEvent::ReviewPassed), Vec::new(), cost)
                    }
                    Ok(Verdict::Fail) => (
                        Some(TaskEvent::ReviewFailed {
                            reason: review.blocking_findings.join("; "),
                        }),
                        Vec::new(),
                        cost,
                    ),
                    Ok(Verdict::Inconclusive) => {
                        // Never coerced either way; a human decides.
                        (None, vec!["reviewer was inconclusive".into()], cost)
                    }
                    Err(faults) => {
                        // A self-contradictory verdict is not evidence of anything, so it must
                        // not be applied in either direction.
                        (None, faults, cost)
                    }
                },
                Err(e) => (None, vec![e.to_string()], None),
            };

            if let Some(event) = event {
                if let Some(current) = run.graph.get(task_id).cloned() {
                    if let Ok(next) = apply(&current, event) {
                        run.graph.set_state(next);
                    }
                }
            } else {
                // Three ways to get here — the reviewer abstained, contradicted itself, or could
                // not be reached — and the operator's question is the same in all of them: no
                // verdict was reached, so a person has to decide. Coercing a pass or a fail from
                // a non-answer is the one thing the review gate exists to prevent.
                let title = run
                    .titles
                    .get(&task_id)
                    .cloned()
                    .unwrap_or_else(|| task_id.to_string());
                run.escalate(
                    EscalationKind::TaskBlocked,
                    Some(task_id),
                    format!("The reviewer reached no verdict on \u{201c}{title}\u{201d}"),
                    errors.join("; "),
                );
            }

            run.log.record_model_decision(
                run.state.iteration,
                Stage::Judge,
                "review_verdict",
                "",
                errors,
                0,
                cost,
            );
            judged += 1;
        }

        // Only claim the run is reviewing if something actually was.
        if judged > 0 && run.state.phase == RunPhase::Monitoring {
            run.state.phase = RunPhase::Reviewing;
        }
    }
}

/// Placeholder until per-call cost is threaded back from the assignment call itself.
fn choice_cost(_eligible: &[AgentId]) -> Option<f64> {
    None
}

/// Records that an agent claimed completion, queueing the task for verification.
///
/// Separate from the pipeline because it is driven by an event from the agent, not by a stage.
pub fn claim_done(run: &mut Run, task_id: TaskId) -> bool {
    let Some(current) = run.graph.get(task_id).cloned() else {
        return false;
    };
    match apply(&current, TaskEvent::ClaimedDone) {
        Ok(next) => {
            run.graph.set_state(next);
            run.awaiting_verification.push(task_id);
            true
        }
        // A session that exits without a legal claim has failed, not finished.
        Err(_) => false,
    }
}
