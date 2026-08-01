//! The deterministic verification gate.
//!
//! This is the anti-false-success mechanism, so the tests are about what it must *refuse* to
//! let through: a red test, a vacuous contract, a hanging command, and work whose files were
//! never created.

use deck_supervisor::contract::{
    run_gate, validate_and_repair, ContractRepair, Criterion, GateOutcome, TaskContract,
    Verification,
};
use std::path::PathBuf;
use std::time::Duration;

fn worktree(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-gate-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn command_criterion(id: &str, cmd: &str) -> Criterion {
    Criterion {
        id: id.into(),
        text: format!("runs {cmd}"),
        verify: Verification::Command {
            cmd: cmd.into(),
            cwd_rel: None,
            expect_exit_zero: true,
        },
    }
}

fn contract(criteria: Vec<Criterion>) -> TaskContract {
    TaskContract {
        version: 1,
        acceptance_criteria: criteria,
        constraints: vec![],
        deliverables: vec![],
        definition_of_done: "done".into(),
    }
}

const TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn all_passing_commands_produce_a_pass() {
    let wt = worktree("pass");
    let c = contract(vec![
        command_criterion("a", "true"),
        command_criterion("b", "exit 0"),
    ]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Passed {
            outcomes,
            judgment_pending,
        } => {
            assert_eq!(outcomes.len(), 2);
            assert!(outcomes.iter().all(|o| o.passed));
            assert_eq!(outcomes[0].exit_code, Some(0));
            assert_eq!(judgment_pending, 0);
        }
        other => panic!("expected Passed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_single_failing_command_fails_the_gate() {
    // The whole point: one red test is enough, and no model gets a say.
    let wt = worktree("fail");
    let c = contract(vec![
        command_criterion("ok", "true"),
        command_criterion("broken", "exit 1"),
    ]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Failed { outcomes } => {
            let broken = outcomes
                .iter()
                .find(|o| o.criterion_id == "broken")
                .unwrap();
            assert!(!broken.passed);
            assert_eq!(broken.exit_code, Some(1));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn judgment_criteria_are_never_evaluated_by_the_gate() {
    // The reviewer judges those, and only once the executable ones pass. If the gate scored
    // them it would be making the model's call for it.
    let wt = worktree("judgment");
    let c = contract(vec![
        command_criterion("tests", "true"),
        Criterion {
            id: "readable".into(),
            text: "the code is readable".into(),
            verify: Verification::Judgment {
                rubric: "is it clear?".into(),
            },
        },
    ]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Passed {
            outcomes,
            judgment_pending,
        } => {
            assert_eq!(outcomes.len(), 1, "only the command should have been run");
            assert_eq!(
                judgment_pending, 1,
                "the reviewer still has something to judge"
            );
        }
        other => panic!("expected Passed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_contract_of_only_judgment_criteria_still_fails_on_a_red_test_after_repair() {
    // A judgment-only contract defines success as "a model agreed", which is exactly what this
    // design exists to prevent. Validation injects a real command, and that command then gates.
    let mut c = contract(vec![Criterion {
        id: "vibes".into(),
        text: "looks good".into(),
        verify: Verification::Judgment {
            rubric: "does it look good?".into(),
        },
    }]);

    assert!(!c.has_executable_criterion());
    let repairs = validate_and_repair(&mut c, Some("exit 1"));
    assert!(matches!(
        repairs.as_slice(),
        [ContractRepair::InjectedDefaultVerification { .. }]
    ));
    assert!(c.has_executable_criterion());

    let wt = worktree("vacuous");
    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Failed { outcomes } => {
            assert!(
                outcomes.iter().any(|o| !o.passed),
                "the injected command must actually gate"
            );
        }
        other => panic!("a vacuous contract must not be able to pass: {other:?}"),
    }
}

#[tokio::test]
async fn validation_leaves_a_contract_that_already_verifies_alone() {
    let mut c = contract(vec![command_criterion("tests", "cargo test")]);
    let before = c.clone();
    let repairs = validate_and_repair(&mut c, Some("cargo test"));

    assert!(repairs.is_empty(), "nothing needed repairing");
    assert_eq!(c, before, "a sound contract must not be rewritten");
}

#[tokio::test]
async fn validation_generates_ids_for_unnamed_criteria() {
    // Criterion ids are how a reviewer's verdict refers back to specific criteria, so a blank
    // one would make the verdict unmappable.
    let mut c = contract(vec![Criterion {
        id: "  ".into(),
        text: "something".into(),
        verify: Verification::Command {
            cmd: "true".into(),
            cwd_rel: None,
            expect_exit_zero: true,
        },
    }]);

    let repairs = validate_and_repair(&mut c, Some("true"));
    assert!(matches!(
        repairs.as_slice(),
        [ContractRepair::GeneratedCriterionId { .. }]
    ));
    assert_eq!(c.acceptance_criteria[0].id, "criterion-1");
}

#[tokio::test]
async fn a_hanging_command_fails_rather_than_being_treated_as_unknown() {
    // A verification command that never finishes is indistinguishable from one that fails.
    // Treating it as inconclusive would let a hanging suite pass as "not yet failed".
    let wt = worktree("hang");
    let c = contract(vec![command_criterion("hangs", "sleep 30")]);

    let started = std::time::Instant::now();
    let outcome = run_gate(&wt, &c, Duration::from_millis(500)).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "the gate must not wait out a hanging command: {elapsed:?}"
    );
    match outcome {
        GateOutcome::Failed { outcomes } => {
            assert!(!outcomes[0].passed);
            assert!(
                outcomes[0].detail.contains("timed out"),
                "the reason should say so: {}",
                outcomes[0].detail
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn commands_run_in_the_worktree_not_the_supervisors_directory() {
    // Verification has to observe the agent's work. Running in the wrong directory would test
    // the wrong tree — and would likely pass, which is the dangerous direction.
    let wt = worktree("cwd");
    std::fs::write(wt.join("marker.txt"), "agent work\n").unwrap();

    let c = contract(vec![command_criterion("sees-file", "test -f marker.txt")]);

    assert!(matches!(
        run_gate(&wt, &c, TIMEOUT).await,
        GateOutcome::Passed { .. }
    ));
}

#[tokio::test]
async fn a_relative_cwd_is_resolved_inside_the_worktree() {
    let wt = worktree("subdir");
    std::fs::create_dir_all(wt.join("crates/thing")).unwrap();
    std::fs::write(wt.join("crates/thing/Cargo.toml"), "[package]\n").unwrap();

    let c = contract(vec![Criterion {
        id: "sub".into(),
        text: "runs in a subdirectory".into(),
        verify: Verification::Command {
            cmd: "test -f Cargo.toml".into(),
            cwd_rel: Some("crates/thing".into()),
            expect_exit_zero: true,
        },
    }]);

    assert!(matches!(
        run_gate(&wt, &c, TIMEOUT).await,
        GateOutcome::Passed { .. }
    ));
}

#[tokio::test]
async fn missing_deliverable_files_fail_the_gate() {
    let wt = worktree("files");
    std::fs::write(wt.join("present.rs"), "//\n").unwrap();

    let c = contract(vec![Criterion {
        id: "files".into(),
        text: "expected files exist".into(),
        verify: Verification::FilesExist {
            globs: vec!["present.rs".into(), "absent.rs".into()],
        },
    }]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Failed { outcomes } => {
            assert!(
                outcomes[0].detail.contains("absent.rs"),
                "the failure should name what is missing: {}",
                outcomes[0].detail
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_trailing_star_pattern_matches_by_prefix() {
    let wt = worktree("glob");
    std::fs::create_dir_all(wt.join("tests")).unwrap();
    std::fs::write(wt.join("tests/auth_test.rs"), "//\n").unwrap();

    let c = contract(vec![Criterion {
        id: "tests-added".into(),
        text: "a test file was added".into(),
        verify: Verification::FilesExist {
            globs: vec!["tests/auth_*".into()],
        },
    }]);

    assert!(matches!(
        run_gate(&wt, &c, TIMEOUT).await,
        GateOutcome::Passed { .. }
    ));
}

#[tokio::test]
async fn a_missing_worktree_is_inconclusive_not_a_failure() {
    // Inconclusive is distinct on purpose: the agent's work was never judged, so this must not
    // consume a review round the way a genuine rejection does.
    let missing = std::env::temp_dir().join(format!("agentdeck-gone-{}", uuid::Uuid::new_v4()));
    let c = contract(vec![command_criterion("tests", "true")]);

    match run_gate(&missing, &c, TIMEOUT).await {
        GateOutcome::Inconclusive { reason } => {
            assert!(reason.contains("missing"), "reason: {reason}");
        }
        other => panic!("expected Inconclusive, got {other:?}"),
    }
}

#[tokio::test]
async fn an_uninvokable_command_fails_with_a_usable_reason() {
    let wt = worktree("nocmd");
    let c = contract(vec![command_criterion(
        "missing-tool",
        "definitely-not-a-real-binary-xyz",
    )]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Failed { outcomes } => {
            assert!(!outcomes[0].passed);
            assert!(
                outcomes[0].exit_code.is_some() || !outcomes[0].detail.is_empty(),
                "a missing tool must be explained, not silently pass"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn output_is_captured_so_the_reviewer_sees_evidence_not_assertions() {
    // The reviewer is shown captured output rather than the developer's claims, which is what
    // makes it unable to be argued into agreeing that a red test is fine.
    let wt = worktree("capture");
    let c = contract(vec![command_criterion(
        "noisy",
        "echo 'assertion failed: 1 == 2'; exit 1",
    )]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Failed { outcomes } => {
            assert!(
                outcomes[0].output_tail.contains("assertion failed"),
                "the failure output must be captured: {:?}",
                outcomes[0].output_tail
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_long_log_is_truncated_from_the_front_keeping_the_failure_summary() {
    // Test runners put the summary last, and an untruncated log would swamp both the reviewer's
    // context and the operator's screen.
    let wt = worktree("longlog");
    let c = contract(vec![command_criterion(
        "verbose",
        "for i in $(seq 1 4000); do echo \"line $i of noise\"; done; echo FINAL_SUMMARY; exit 1",
    )]);

    match run_gate(&wt, &c, TIMEOUT).await {
        GateOutcome::Failed { outcomes } => {
            let tail = &outcomes[0].output_tail;
            assert!(
                tail.len() < 3_000,
                "output should be bounded, got {}",
                tail.len()
            );
            assert!(
                tail.contains("FINAL_SUMMARY"),
                "the end of the log is the useful part"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn a_criterion_can_require_a_command_to_fail() {
    // For criteria like "the old endpoint is gone".
    let wt = worktree("expectfail");
    let c = contract(vec![Criterion {
        id: "removed".into(),
        text: "the legacy file is gone".into(),
        verify: Verification::Command {
            cmd: "test -f legacy.rs".into(),
            cwd_rel: None,
            expect_exit_zero: false,
        },
    }]);

    assert!(matches!(
        run_gate(&wt, &c, TIMEOUT).await,
        GateOutcome::Passed { .. }
    ));

    std::fs::write(wt.join("legacy.rs"), "//\n").unwrap();
    assert!(matches!(
        run_gate(&wt, &c, TIMEOUT).await,
        GateOutcome::Failed { .. }
    ));
}
