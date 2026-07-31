//! Creating, inspecting and retiring agent worktrees.

use super::{git, repo_root, GitError, RepoLocks, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where agent worktrees live inside the target repository.
pub const WORKTREE_DIR: &str = ".agentdeck";

#[derive(Debug, Clone)]
pub struct WorktreeSpec {
    pub agent_slug: String,
    pub task_short_id: String,
    pub task_title: String,
    /// Branch or ref to start from. Resolved to a commit at creation time.
    pub base_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
    pub base_ref: String,
    /// The commit the worktree started from, pinned at creation.
    ///
    /// Recorded instead of the branch name because `main` moves. A review diff computed against
    /// a moving ref would silently change meaning between the developer finishing and the
    /// reviewer looking.
    pub base_sha: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorktreeStatus {
    pub path: PathBuf,
    pub branch: String,
    /// True when there is anything uncommitted, staged or untracked.
    pub dirty: bool,
    pub changed_files: Vec<String>,
    pub commits_ahead_of_base: u32,
}

pub struct WorktreeManager {
    locks: RepoLocks,
}

impl Default for WorktreeManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WorktreeManager {
    pub fn new() -> Self {
        Self {
            locks: RepoLocks::new(),
        }
    }

    /// Creates the worktree for a task, or returns the existing one.
    ///
    /// Idempotent so a retry or a crash-recovery pass does not fail on an already-created tree.
    pub async fn ensure(&self, repo: &Path, spec: &WorktreeSpec) -> Result<WorktreeInfo> {
        let root = repo_root(repo).await?;
        let lock = self.locks.for_repo(&root);
        let _guard = lock.lock().await;

        let path = worktree_path(&root, &spec.agent_slug, &spec.task_short_id);
        let branch = branch_name(&spec.agent_slug, &spec.task_short_id, &spec.task_title);

        // Resolve to a commit before creating anything, so the recorded base cannot drift.
        let base_sha = git(&root, &["rev-parse", &spec.base_ref]).await?;

        if path.exists() {
            let existing = WorktreeInfo {
                path: path.clone(),
                branch: current_branch(&path).await.unwrap_or(branch),
                base_ref: spec.base_ref.clone(),
                base_sha: merge_base(&root, &path).await.unwrap_or(base_sha),
            };
            return Ok(existing);
        }

        // Excluded via .git/info/exclude rather than .gitignore: adding worktrees to a tracked
        // file would be an uninvited change to the user's repository.
        exclude_agentdeck_dir(&root).await?;

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let path_str = path.to_string_lossy().to_string();
        git(
            &root,
            &["worktree", "add", "-b", &branch, &path_str, &base_sha],
        )
        .await?;

        Ok(WorktreeInfo {
            path,
            branch,
            base_ref: spec.base_ref.clone(),
            base_sha,
        })
    }

    pub async fn status(&self, repo: &Path, info: &WorktreeInfo) -> Result<WorktreeStatus> {
        let root = repo_root(repo).await?;
        let lock = self.locks.for_repo(&root);
        let _guard = lock.lock().await;

        if !info.path.exists() {
            return Err(GitError::UnknownWorktree {
                path: info.path.clone(),
            });
        }

        // porcelain=v1 with -uall so untracked files count as dirty. An agent that created files
        // without committing has still done work worth protecting.
        let porcelain = git(&info.path, &["status", "--porcelain", "-uall"]).await?;
        let changed_files: Vec<String> = porcelain
            .lines()
            .filter_map(|l| l.get(3..).map(str::to_string))
            .collect();

        let ahead = git(
            &info.path,
            &["rev-list", "--count", &format!("{}..HEAD", info.base_sha)],
        )
        .await
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);

        Ok(WorktreeStatus {
            path: info.path.clone(),
            branch: current_branch(&info.path)
                .await
                .unwrap_or_else(|| info.branch.clone()),
            dirty: !changed_files.is_empty(),
            changed_files,
            commits_ahead_of_base: ahead,
        })
    }

    /// Diff from the pinned base to the worktree's HEAD.
    pub async fn diff(&self, repo: &Path, info: &WorktreeInfo) -> Result<String> {
        let root = repo_root(repo).await?;
        let lock = self.locks.for_repo(&root);
        let _guard = lock.lock().await;

        // Three-dot: what the agent added, excluding anything that landed on the base since.
        git(&info.path, &["diff", &format!("{}...HEAD", info.base_sha)]).await
    }

    /// Removes a worktree.
    ///
    /// Refuses while dirty. The worktree holds the only copy of uncommitted work, and unlike
    /// most git operations this one is not recoverable.
    ///
    /// **Also destroys session resumability.** Claude buckets transcripts by working directory,
    /// so every session created in this worktree becomes unresumable once it is gone. Callers
    /// should surface that, not treat removal as mere disk cleanup.
    pub async fn remove(&self, repo: &Path, info: &WorktreeInfo, force: bool) -> Result<()> {
        let status = self.status(repo, info).await?;
        if status.dirty && !force {
            return Err(GitError::DirtyWorktree {
                path: info.path.clone(),
            });
        }

        let root = repo_root(repo).await?;
        let lock = self.locks.for_repo(&root);
        let _guard = lock.lock().await;

        let path_str = info.path.to_string_lossy().to_string();
        let mut args = vec!["worktree", "remove"];
        // Only ever on an explicit caller request, never as an automatic retry.
        if force {
            args.push("--force");
        }
        args.push(&path_str);

        git(&root, &args).await?;
        Ok(())
    }

    /// Lists the worktrees git actually knows about.
    pub async fn list(&self, repo: &Path) -> Result<Vec<PathBuf>> {
        let root = repo_root(repo).await?;
        let lock = self.locks.for_repo(&root);
        let _guard = lock.lock().await;

        let out = git(&root, &["worktree", "list", "--porcelain"]).await?;
        Ok(out
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(PathBuf::from)
            .collect())
    }

    /// Boot-time reconciliation: drops git's records of worktrees whose directories are gone,
    /// then reports what remains. The app's own records are reconciled against this, because
    /// after a crash the database and the filesystem will disagree.
    pub async fn prune_and_list(&self, repo: &Path) -> Result<Vec<PathBuf>> {
        let root = repo_root(repo).await?;
        {
            let lock = self.locks.for_repo(&root);
            let _guard = lock.lock().await;
            git(&root, &["worktree", "prune"]).await?;
        }
        self.list(repo).await
    }
}

fn worktree_path(root: &Path, agent_slug: &str, task_short_id: &str) -> PathBuf {
    root.join(WORKTREE_DIR)
        .join(format!("agent-{agent_slug}"))
        .join(format!("task-{task_short_id}"))
}

/// Branch names are task-scoped, so a retry gets a clean branch and the history of each attempt
/// stays separately inspectable.
pub fn branch_name(agent_slug: &str, task_short_id: &str, task_title: &str) -> String {
    let slug = slugify(task_title);
    if slug.is_empty() {
        format!("agentdeck/{agent_slug}/{task_short_id}")
    } else {
        format!("agentdeck/{agent_slug}/{task_short_id}-{slug}")
    }
}

/// Conservative slug: git refs reject a long list of characters and sequences, so anything
/// outside a safe set becomes a dash.
fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }

    out.trim_matches('-').to_string()
}

async fn current_branch(worktree: &Path) -> Option<String> {
    git(worktree, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await
        .ok()
        .filter(|b| b != "HEAD")
}

async fn merge_base(root: &Path, worktree: &Path) -> Option<String> {
    let head = git(worktree, &["rev-parse", "HEAD"]).await.ok()?;
    git(root, &["merge-base", "HEAD", &head]).await.ok()
}

/// Adds the worktree directory to `.git/info/exclude`, which is local-only and untracked.
async fn exclude_agentdeck_dir(root: &Path) -> Result<()> {
    let exclude = root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let existing = tokio::fs::read_to_string(&exclude)
        .await
        .unwrap_or_default();
    let entry = format!("/{WORKTREE_DIR}/");
    if existing.lines().any(|l| l.trim() == entry) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("# AgentDeck agent worktrees (local only)\n");
    updated.push_str(&entry);
    updated.push('\n');

    tokio::fs::write(&exclude, updated).await?;
    Ok(())
}
