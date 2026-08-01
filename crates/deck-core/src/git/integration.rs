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
    /// Everything merged, but the project has no test command to run against it.
    ///
    /// Distinct from `Integrated` because it is a materially weaker claim, and distinct from
    /// `Inconclusive` because the merge itself did succeed and there is nothing to retry.
    MergedUnverified { merged: Vec<String>, reason: String },
    /// The integration could not be attempted. Not a verdict on the work.
    Inconclusive { reason: String },
}

impl WorktreeManager {
    /// Merges every contribution into a scratch worktree and runs `test_command` there.
    ///
    /// A `None` command means the project's toolchain was not recognised. The merge still runs —
    /// conflict detection is worth having on its own — but the result is reported as unverified
    /// rather than passing.
    ///
    /// The integration tree is disposable and detached: a run must not leave the operator's own
    /// checked-out branch holding a merge they never asked for, and a failed attempt must leave
    /// nothing behind for them to undo.
    pub async fn integrate(
        &self,
        repo: &Path,
        base_ref: &str,
        contributions: &[Contribution],
        test_command: Option<&str>,
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

        let Some(test_command) = test_command else {
            return Ok(IntegrationOutcome::MergedUnverified {
                merged,
                reason: "no test command could be identified for this project".into(),
            });
        };

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

/// What happened when the integrated result was moved onto the project's own branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LandOutcome {
    /// The branch now points at the integrated commit, and the files are in the working tree.
    Landed { branch: String, commit: String },
    /// Deliberately not done. Never a failure of the work — a reason the operator must resolve.
    Refused { reason: String },
}

impl WorktreeManager {
    /// Moves a successful integration onto the branch the operator actually has checked out.
    ///
    /// Without this a finished run left the project directory exactly as it started: every
    /// deliverable existed, but only on task branches and inside a scratch worktree, so the
    /// obvious reading was that the agents had produced nothing. Integration proving the branches
    /// merge and pass is precisely the point at which the result has earned a place on the branch.
    ///
    /// Refuses rather than forces, in every case where landing could destroy something:
    ///
    /// - the working tree has uncommitted changes — they would be overwritten by the checkout
    /// - the branch has moved since integration started — a fast-forward would be a rewrite, and
    ///   the integration was never tested against whatever arrived in the meantime
    /// - the branch is checked out in another worktree, where updating it would surprise whoever
    ///   is using it
    ///
    /// Every refusal leaves the integration worktree intact, so nothing is lost and the operator
    /// can merge by hand.
    pub async fn land(&self, repo: &Path, base_ref: &str) -> Result<LandOutcome> {
        let root = repo_root(repo).await?;
        let lock = self.locks_for(&root);
        let _guard = lock.lock().await;

        let path = integration_path(&root);
        if !path.is_dir() {
            return Ok(LandOutcome::Refused {
                reason: "there is no integration worktree to land".into(),
            });
        }

        let integrated = git(&path, &["rev-parse", "HEAD"]).await?.trim().to_string();

        // Ancestry rather than a remembered sha. The question that matters is whether
        // fast-forwarding would lose a commit, and git answers that directly — a recorded sha only
        // answers "is this identical to when we started", which is stricter and different. It also
        // lets work be landed long after the run that produced it, which is exactly the case that
        // matters when a run blocks and its output is stranded in the integration worktree.
        let current = git(&root, &["rev-parse", base_ref])
            .await?
            .trim()
            .to_string();

        if current == integrated {
            return Ok(LandOutcome::Refused {
                reason: format!("{base_ref} is already at the integrated commit"),
            });
        }

        if git(
            &root,
            &["merge-base", "--is-ancestor", &current, &integrated],
        )
        .await
        .is_err()
        {
            return Ok(LandOutcome::Refused {
                reason: format!(
                    "{base_ref} has commits the integration does not contain, so landing would \
                     discard them; merge the integration worktree by hand"
                ),
            });
        }

        // Our own worktrees are filtered out by path rather than trusted to be excluded. They
        // live under `.agentdeck`, which is normally in `.git/info/exclude` — but that entry is
        // written when the first agent worktree is created, and a refusal to land is far too
        // consequential to rest on something written elsewhere for another reason.
        let status = git(&root, &["status", "--porcelain"]).await?;
        let dirty: Vec<&str> = status
            .lines()
            .filter(|line| {
                let path = line.get(3..).unwrap_or("").trim_matches('"');
                !path.starts_with(".agentdeck/") && path != ".agentdeck"
            })
            .collect();

        if !dirty.is_empty() {
            return Ok(LandOutcome::Refused {
                reason: "you have uncommitted changes; commit or stash them and land by hand"
                    .into(),
            });
        }

        let head = git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await
            .unwrap_or_default()
            .trim()
            .to_string();

        if head == base_ref {
            // Fast-forward the checked-out branch, which also updates the files on disk. `merge
            // --ff-only` rather than `reset --hard`: it refuses instead of discarding if the
            // relationship is not what we believe it is.
            git(&root, &["merge", "--ff-only", &integrated]).await?;
        } else {
            // Not checked out here. Updating the ref is enough and touches no working tree.
            git(&root, &["branch", "--force", base_ref, &integrated]).await?;
        }

        Ok(LandOutcome::Landed {
            branch: base_ref.to_string(),
            commit: integrated,
        })
    }
}

/// The commit a ref currently points at.
pub async fn head_sha(repo: &Path, git_ref: &str) -> Result<String> {
    let root = repo_root(repo).await?;
    Ok(git(&root, &["rev-parse", git_ref])
        .await?
        .trim()
        .to_string())
}
