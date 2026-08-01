//! Task contracts and the deterministic verification gate.
//!
//! This is the load-bearing anti-false-success mechanism. The supervisor runs every executable
//! acceptance criterion **itself**, in the agent's worktree, and captures exit codes. If any
//! fails, the verdict is `fail` and the reviewer is never invoked.
//!
//! That ordering is deliberate and matters for two reasons. It saves a model call on work that
//! is provably not done — but more importantly it makes the reviewer *structurally unable* to
//! override a red test. A reviewer that saw "the developer says tests pass" could be argued into
//! agreeing. A reviewer that only ever sees captured exit codes cannot.
//!
//! The other half is contract validation: a contract with no executable criterion is vacuous,
//! and accepting one would let a planner define success as "the agent said so". Validation
//! injects the project's default test command instead of accepting that.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Default ceiling for a single verification command. A hung test run must not stall the
/// supervisor loop indefinitely.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Verification {
    /// Run a command and check its exit status. The supervisor executes this, never the agent.
    Command {
        cmd: String,
        #[serde(default)]
        cwd_rel: Option<String>,
        #[serde(default = "default_true")]
        expect_exit_zero: bool,
    },
    /// Files that must exist relative to the worktree root.
    FilesExist { globs: Vec<String> },
    /// Requires a reviewer's judgement. Only reachable once every executable criterion passes.
    Judgment { rubric: String },
}

fn default_true() -> bool {
    true
}

impl Verification {
    /// Whether this criterion can be checked without a model.
    pub fn is_executable(&self) -> bool {
        matches!(
            self,
            Verification::Command { .. } | Verification::FilesExist { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Criterion {
    pub id: String,
    pub text: String,
    pub verify: Verification,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Constraint {
    /// Paths the agent must not touch, checked against the diff after the fact.
    PathsForbidden {
        globs: Vec<String>,
    },
    MaxDiffLines {
        max: u32,
    },
    /// Judged by the reviewer; not machine-checkable.
    Textual {
        text: String,
    },
}

/// Every field defaults, so a planner that omits parts of the contract still produces a parseable
/// response. An unparseable one would be indistinguishable from a model failure and would skip the
/// repair round-trip that exists precisely to correct omissions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskContract {
    pub version: u32,
    pub acceptance_criteria: Vec<Criterion>,
    pub constraints: Vec<Constraint>,
    pub deliverables: Vec<String>,
    pub definition_of_done: String,
}

impl TaskContract {
    pub fn executable_criteria(&self) -> impl Iterator<Item = &Criterion> {
        self.acceptance_criteria
            .iter()
            .filter(|c| c.verify.is_executable())
    }

    pub fn has_executable_criterion(&self) -> bool {
        self.executable_criteria().next().is_some()
    }
}

/// What a contract had to be corrected on, so the decision log records that the planner's
/// output was insufficient rather than silently improving it.
#[derive(Debug, Clone, PartialEq)]
pub enum ContractRepair {
    /// A default test command was added because the planner supplied no executable criterion.
    InjectedDefaultVerification { cmd: String },
    /// The contract had no executable criterion and the project offered no command to inject.
    NoVerificationAvailable,
    /// A criterion had an empty id and was given a generated one.
    GeneratedCriterionId { id: String },
}

/// Ensures a contract can actually be verified.
///
/// A contract whose only criteria are `Judgment` defines success as "a model agreed", which is
/// exactly the failure this whole design exists to prevent. Rather than reject and re-prompt,
/// code injects the project's default test command — the planner omitting it is a predictable
/// gap, not a reason to burn another round-trip.
pub fn validate_and_repair(
    contract: &mut TaskContract,
    default_test_command: Option<&str>,
) -> Vec<ContractRepair> {
    let mut repairs = Vec::new();

    for (index, criterion) in contract.acceptance_criteria.iter_mut().enumerate() {
        if criterion.id.trim().is_empty() {
            let id = format!("criterion-{}", index + 1);
            criterion.id = id.clone();
            repairs.push(ContractRepair::GeneratedCriterionId { id });
        }
    }

    if !contract.has_executable_criterion() {
        // Recorded rather than papered over. Leaving the contract on judgment criteria alone is
        // the weaker outcome this function exists to avoid, so when it is unavoidable it belongs
        // in the decision log where an operator can see why nothing was executable.
        let Some(default_test_command) = default_test_command else {
            repairs.push(ContractRepair::NoVerificationAvailable);
            return repairs;
        };

        contract.acceptance_criteria.push(Criterion {
            id: "injected-default-verification".into(),
            text: format!("The project's tests pass (`{default_test_command}`)"),
            verify: Verification::Command {
                cmd: default_test_command.to_string(),
                cwd_rel: None,
                expect_exit_zero: true,
            },
        });
        repairs.push(ContractRepair::InjectedDefaultVerification {
            cmd: default_test_command.to_string(),
        });
    }

    repairs
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionOutcome {
    pub criterion_id: String,
    pub passed: bool,
    /// `None` for non-command criteria.
    pub exit_code: Option<i32>,
    /// Trimmed output, kept short: this is shown to the reviewer and to the operator, and a
    /// full test log would swamp both.
    pub output_tail: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateOutcome {
    /// Every executable criterion passed. Judgment criteria remain for the reviewer.
    Passed {
        outcomes: Vec<CriterionOutcome>,
        judgment_pending: usize,
    },
    /// At least one executable criterion failed. The reviewer is not invoked.
    Failed { outcomes: Vec<CriterionOutcome> },
    /// The contract could not be verified at all — a missing worktree, for example. Distinct
    /// from `Failed` because the agent's work has not been judged, so it must not count as a
    /// review round.
    Inconclusive { reason: String },
}

const OUTPUT_TAIL_BYTES: usize = 2_000;

/// Runs every executable criterion in the worktree.
///
/// Deliberately takes the worktree path rather than a session, so verification cannot be
/// influenced by the agent's process, environment or conversation.
pub async fn run_gate(worktree: &Path, contract: &TaskContract, timeout: Duration) -> GateOutcome {
    if !worktree.exists() {
        return GateOutcome::Inconclusive {
            reason: format!(
                "worktree {} is missing, so the work could not be verified",
                worktree.display()
            ),
        };
    }

    let mut outcomes = Vec::new();

    for criterion in contract.executable_criteria() {
        let outcome = match &criterion.verify {
            Verification::Command {
                cmd,
                cwd_rel,
                expect_exit_zero,
            } => {
                let cwd = match cwd_rel {
                    Some(rel) => worktree.join(rel),
                    None => worktree.to_path_buf(),
                };
                run_command(&criterion.id, cmd, &cwd, *expect_exit_zero, timeout).await
            }
            Verification::FilesExist { globs } => check_files_exist(&criterion.id, worktree, globs),
            // Filtered out by executable_criteria(), but matched exhaustively so adding a variant
            // is a compile error rather than a silently skipped check.
            Verification::Judgment { .. } => continue,
        };
        outcomes.push(outcome);
    }

    let judgment_pending = contract
        .acceptance_criteria
        .iter()
        .filter(|c| matches!(c.verify, Verification::Judgment { .. }))
        .count();

    if outcomes.iter().all(|o| o.passed) {
        GateOutcome::Passed {
            outcomes,
            judgment_pending,
        }
    } else {
        GateOutcome::Failed { outcomes }
    }
}

async fn run_command(
    criterion_id: &str,
    cmd: &str,
    cwd: &Path,
    expect_exit_zero: bool,
    timeout: Duration,
) -> CriterionOutcome {
    // Through a shell, because acceptance commands are written as shell one-liners. This is the
    // supervisor's own command from a contract it validated, not agent-supplied input.
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .env("CI", "1")
        .kill_on_drop(true);

    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(timeout, command.output()).await;

    match result {
        Ok(Ok(output)) => {
            let code = output.status.code();
            let succeeded = output.status.success();
            let passed = if expect_exit_zero {
                succeeded
            } else {
                !succeeded
            };

            let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&output.stderr));

            CriterionOutcome {
                criterion_id: criterion_id.to_string(),
                passed,
                exit_code: code,
                output_tail: tail(&combined),
                detail: format!(
                    "`{cmd}` exited {} after {:?}",
                    code.map(|c| c.to_string())
                        .unwrap_or_else(|| "signal".into()),
                    started.elapsed()
                ),
            }
        }
        Ok(Err(e)) => CriterionOutcome {
            criterion_id: criterion_id.to_string(),
            passed: false,
            exit_code: None,
            output_tail: String::new(),
            detail: format!("`{cmd}` could not be started: {e}"),
        },
        Err(_) => CriterionOutcome {
            criterion_id: criterion_id.to_string(),
            passed: false,
            exit_code: None,
            output_tail: String::new(),
            // A timeout is a failure, not inconclusive: a verification command that never
            // finishes is indistinguishable from one that fails, and treating it as unknown
            // would let a hanging test suite pass as "not yet failed".
            detail: format!("`{cmd}` timed out after {timeout:?}"),
        },
    }
}

fn check_files_exist(criterion_id: &str, worktree: &Path, globs: &[String]) -> CriterionOutcome {
    // Literal relative paths and a single trailing `*` are supported; full glob syntax is not,
    // and an unmatched pattern fails rather than being skipped.
    let mut missing = Vec::new();

    for pattern in globs {
        if !matches_any(worktree, pattern) {
            missing.push(pattern.clone());
        }
    }

    CriterionOutcome {
        criterion_id: criterion_id.to_string(),
        passed: missing.is_empty(),
        exit_code: None,
        output_tail: String::new(),
        detail: if missing.is_empty() {
            format!("all {} expected path(s) present", globs.len())
        } else {
            format!("missing: {}", missing.join(", "))
        },
    }
}

fn matches_any(worktree: &Path, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        let (dir, stem) = match prefix.rsplit_once('/') {
            Some((d, s)) => (worktree.join(d), s.to_string()),
            None => (worktree.to_path_buf(), prefix.to_string()),
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        return entries.filter_map(|e| e.ok()).any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(stem.as_str())
        });
    }

    worktree.join(pattern).exists()
}

fn tail(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= OUTPUT_TAIL_BYTES {
        return trimmed.to_string();
    }
    // Keep the end: test runners put the failure summary last.
    let start = trimmed.len() - OUTPUT_TAIL_BYTES;
    let boundary = trimmed
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= start)
        .unwrap_or(0);
    format!("…{}", &trimmed[boundary..])
}
