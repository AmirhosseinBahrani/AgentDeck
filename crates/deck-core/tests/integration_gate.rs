//! Merging the agents' branches back together.
//!
//! These run against real git repositories, because the thing being tested is what git actually
//! does with two divergent branches — a fake would only test our belief about that, and the
//! belief is the part most likely to be wrong.
//!
//! The scenario throughout: two agents each finished a task, each passed its own verification in
//! its own worktree, and neither has ever seen the other's work.

use deck_core::git::integration::{integration_path, Contribution};
use deck_core::git::{IntegrationOutcome, WorktreeManager};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn sh(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scratch_repo(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-int-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    sh(&dir, &["init", "-q", "-b", "main"]);
    sh(&dir, &["config", "user.name", "Test"]);
    sh(&dir, &["config", "user.email", "t@example.com"]);
    std::fs::write(dir.join("shared.txt"), "line one\n").unwrap();
    sh(&dir, &["add", "."]);
    sh(&dir, &["commit", "-q", "-m", "initial"]);
    dir
}

/// Commits `content` to `file` on a new branch off main, the way a finished agent leaves things.
fn branch_with(repo: &Path, branch: &str, file: &str, content: &str) -> Contribution {
    sh(repo, &["checkout", "-q", "-b", branch, "main"]);
    std::fs::write(repo.join(file), content).unwrap();
    sh(repo, &["add", "."]);
    sh(repo, &["commit", "-q", "-m", &format!("work on {branch}")]);
    sh(repo, &["checkout", "-q", "main"]);
    Contribution {
        task_id: branch.to_string(),
        branch: branch.to_string(),
    }
}

#[tokio::test]
async fn independent_branches_merge_and_the_project_tests_run_against_the_result() {
    let repo = scratch_repo("clean");
    let a = branch_with(&repo, "agent-a", "a.txt", "from a\n");
    let b = branch_with(&repo, "agent-b", "b.txt", "from b\n");

    let manager = WorktreeManager::new();
    let outcome = manager
        // Asserts both files are present, so a merge that dropped one would fail here rather
        // than passing quietly.
        .integrate(
            &repo,
            "main",
            &[a, b],
            "test -f a.txt && test -f b.txt",
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    match outcome {
        IntegrationOutcome::Integrated { merged } => assert_eq!(merged.len(), 2),
        other => panic!("expected a clean integration, got {other:?}"),
    }
}

#[tokio::test]
async fn overlapping_work_is_reported_rather_than_resolved() {
    // Two agents edited the same line. Choosing a winner would mean silently discarding one
    // agent's work, so the run stops and says which branch and which file.
    let repo = scratch_repo("conflict");
    let a = branch_with(&repo, "agent-a", "shared.txt", "a's version\n");
    let b = branch_with(&repo, "agent-b", "shared.txt", "b's version\n");

    let manager = WorktreeManager::new();
    let outcome = manager
        .integrate(&repo, "main", &[a, b], "true", Duration::from_secs(30))
        .await
        .unwrap();

    match outcome {
        IntegrationOutcome::Conflicted { branch, files, .. } => {
            assert_eq!(
                branch, "agent-b",
                "the second branch is the one that clashed"
            );
            assert!(
                files.contains(&"shared.txt".to_string()),
                "the conflicting file must be named, got {files:?}"
            );
        }
        other => panic!("expected a conflict, got {other:?}"),
    }

    // And nothing is left mid-merge for the next attempt to trip over.
    let head = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(integration_path(&repo))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&head.stdout).trim().is_empty(),
        "the integration worktree should be clean after aborting the merge"
    );
}

#[tokio::test]
async fn branches_that_merge_but_do_not_work_together_fail_the_gate() {
    // The failure per-task verification cannot see: both branches are green alone, they merge
    // without conflict, and the combination is broken. This is the whole reason the gate exists.
    let repo = scratch_repo("incompatible");
    let a = branch_with(&repo, "agent-a", "a.txt", "from a\n");
    let b = branch_with(&repo, "agent-b", "b.txt", "from b\n");

    let manager = WorktreeManager::new();
    let outcome = manager
        // Stands in for a test suite that only fails once both changes are present.
        .integrate(
            &repo,
            "main",
            &[a, b],
            "test -f a.txt && test -f b.txt && echo 'incompatible' && exit 1",
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    match outcome {
        IntegrationOutcome::TestsFailed { output } => {
            assert!(
                output.contains("incompatible"),
                "the operator needs the output to act on, got {output:?}"
            );
        }
        other => panic!("expected failing tests, got {other:?}"),
    }
}

#[tokio::test]
async fn a_hung_test_suite_is_inconclusive_rather_than_a_failure() {
    // Calling a timeout a failure would blame the agents for an environment problem and send
    // perfectly good work back for rework.
    let repo = scratch_repo("timeout");
    let a = branch_with(&repo, "agent-a", "a.txt", "from a\n");

    let manager = WorktreeManager::new();
    let outcome = manager
        .integrate(&repo, "main", &[a], "sleep 30", Duration::from_millis(300))
        .await
        .unwrap();

    assert!(
        matches!(outcome, IntegrationOutcome::Inconclusive { .. }),
        "expected inconclusive, got {outcome:?}"
    );
}

#[tokio::test]
async fn nothing_to_integrate_is_inconclusive_rather_than_success() {
    // A run with no branches has proven nothing. Reporting success would let an objective with
    // no actual output be declared complete.
    let repo = scratch_repo("empty");
    let manager = WorktreeManager::new();
    let outcome = manager
        .integrate(&repo, "main", &[], "true", Duration::from_secs(5))
        .await
        .unwrap();

    assert!(matches!(outcome, IntegrationOutcome::Inconclusive { .. }));
}

#[tokio::test]
async fn a_second_attempt_starts_from_the_base_rather_than_the_last_one() {
    // A leftover integration tree would carry the previous attempt's merges, so a conflict fixed
    // on one branch would still appear on the retry — and the operator would have no way to tell
    // that they had actually fixed it.
    let repo = scratch_repo("rerun");
    let a = branch_with(&repo, "agent-a", "a.txt", "from a\n");
    let manager = WorktreeManager::new();

    manager
        .integrate(
            &repo,
            "main",
            std::slice::from_ref(&a),
            "true",
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    let b = branch_with(&repo, "agent-b", "b.txt", "from b\n");
    let outcome = manager
        // a.txt must be present because agent-a is merged again, not because it survived from
        // the previous run.
        .integrate(
            &repo,
            "main",
            &[a, b],
            "test -f a.txt && test -f b.txt",
            Duration::from_secs(30),
        )
        .await
        .unwrap();

    match outcome {
        IntegrationOutcome::Integrated { merged } => assert_eq!(merged.len(), 2),
        other => panic!("expected a clean re-integration, got {other:?}"),
    }
}
