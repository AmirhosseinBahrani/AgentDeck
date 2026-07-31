//! Bringing every agent's branch back together.
//!
//! Each task is done in its own worktree on its own branch, which is what lets several agents
//! work at once without fighting over one index. The cost is that "every task passed" only ever
//! means every task passed *alone*. Two branches can each be green and still be incompatible —
//! one renames a function the other calls, both add a field to the same struct, both rewrite the
//! same import block. No per-task verification can see that, by construction.
//!
//! So this is the last gate before a run may call itself complete: merge the branches into one
//! tree and run the project's own tests there. Without it, "completed" means handing the
//! operator a pile of branches and the merge conflict.
//!
//! Conflicts are never resolved automatically, by us or by a model. A conflict means two agents
//! were given overlapping work, and the useful response is to say so — not to guess which side
//! was right and silently discard the other.

use super::{git, repo_root, GitError, Result, WorktreeManager};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// A branch produced by one task, in the order it should be merged.
#[derive(Debug, Clone, PartialEq)]
pub struct Contribution {
    pub task_id: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IntegrationOutcome {
    /// Every branch merged and the project's tests passed against the combined result.
    Integrated { merged: Vec<String> },
    /// Two agents produced incompatible work. Reported, never resolved for them.
    Conflicted {
        branch: String,
        task_id: String,
        files: Vec<String>,
    },
    /// Everything merged, but the combined result does not work — the failure per-task
    /// verification is structurally unable to catch.
    TestsFailed { output: String },
    /// The integration could not be attempted. Not a verdict on the work.
    Inconclusive { reason: String },
}

impl WorktreeManager {
    /// Merges every contribution into a scratch worktree and runs `test_command` there.
    ///
    /// The integration tree is disposable and detached: a run must not leave the operator's own
    /// checked-out branch holding a merge they never asked for, and a failed attempt must leave
    /// nothing behind for them to undo.
    pub async fn integrate(
        &self,
        repo: &Path,
        base_ref: &str,
        contributions: &[Contribution],
        test_command: &str,
        timeout: Duration,
    ) -> Result<IntegrationOutcome> {
        if contributions.is_empty() {
            return Ok(IntegrationOutcome::Inconclusive {
                reason: "no completed task produced a branch to integrate".into(),
            });
        }

        let root = repo_root(repo).await?;
        let lock = self.locks_for(&root);
        let _guard = lock.lock().await;

        let path = integration_path(&root);
        let path_str = path.to_string_lossy().to_string();

        // A tree left by a previous attempt would start the merge from the wrong commit, so it
        // is replaced rather than reused. `--force` is acceptable here and nowhere else: this
        // directory is ours, disposable, and never holds an agent's only copy of anything.
        let _ = git(&root, &["worktree", "remove", "--force", &path_str]).await;
        let _ = tokio::fs::remove_dir_all(&path).await;

        if let Err(e) = git(&root, &["worktree", "add", "--detach", &path_str, base_ref]).await {
            return Ok(IntegrationOutcome::Inconclusive {
                reason: format!("could not create an integration worktree: {e}"),
            });
        }

        let mut merged = Vec::new();
        for contribution in contributions {
            // --no-ff so every contribution stays a distinguishable merge commit. A fast-forward
            // would erase which agent produced what, and that is the first question anyone asks
            // when the combined result misbehaves.
            let attempt = git(
                &path,
                &["merge", "--no-ff", "--no-edit", &contribution.branch],
            )
            .await;

            if attempt.is_err() {
                let files = conflicted_files(&path).await;
                // Abort so the tree is not left mid-merge. Whether the caller retries or gives
                // up, a half-merged worktree helps nobody.
                let _ = git(&path, &["merge", "--abort"]).await;
                return Ok(IntegrationOutcome::Conflicted {
                    branch: contribution.branch.clone(),
                    task_id: contribution.task_id.clone(),
                    files,
                });
            }
            merged.push(contribution.branch.clone());
        }

        match run_tests(&path, test_command, timeout).await {
            TestRun::Passed => Ok(IntegrationOutcome::Integrated { merged }),
            TestRun::Failed(output) => Ok(IntegrationOutcome::TestsFailed { output }),
            TestRun::CouldNotRun(reason) => Ok(IntegrationOutcome::Inconclusive { reason }),
        }
    }
}

enum TestRun {
    Passed,
    Failed(String),
    /// Separate from `Failed` on purpose: an environment problem must not be reported as the
    /// agents having produced broken work, or good work gets sent back for rework.
    CouldNotRun(String),
}

async fn run_tests(dir: &Path, command: &str, timeout: Duration) -> TestRun {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return TestRun::CouldNotRun(format!("could not run {command:?}: {e}")),
        Err(_) => {
            return TestRun::CouldNotRun(format!(
                "{command:?} did not finish within {}s",
                timeout.as_secs()
            ))
        }
    };

    if output.status.success() {
        return TestRun::Passed;
    }

    let mut detail = String::from_utf8_lossy(&output.stdout).into_owned();
    detail.push_str(&String::from_utf8_lossy(&output.stderr));
    // Tail rather than head: a test runner puts its verdict at the end, and the start is
    // compiler noise nobody reads.
    TestRun::Failed(tail(&detail, 4_000))
}

async fn conflicted_files(dir: &Path) -> Vec<String> {
    // `git diff` exits non-zero here for reasons unrelated to the answer, so a failure yields an
    // empty list rather than masking the conflict itself.
    match git(dir, &["diff", "--name-only", "--diff-filter=U"]).await {
        Ok(out) => out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    // Advanced to a char boundary, so a multi-byte character cut in half cannot produce
    // invalid UTF-8.
    let start = (text.len() - max..text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    format!("…\n{}", &text[start..])
}

/// Where the integration worktree lives, so the operator can be pointed at it.
pub fn integration_path(repo: &Path) -> PathBuf {
    repo.join(".agentdeck").join("integration")
}

/// Removes the integration worktree.
pub async fn discard_integration(repo: &Path) -> Result<()> {
    let root = repo_root(repo).await?;
    let path = integration_path(&root).to_string_lossy().to_string();
    git(&root, &["worktree", "remove", "--force", &path])
        .await
        .map(|_| ())
        .map_err(|e| match e {
            // Nothing there is the goal, not a failure.
            GitError::CommandFailed { .. } => GitError::UnknownWorktree {
                path: PathBuf::from(path),
            },
            other => other,
        })
}
