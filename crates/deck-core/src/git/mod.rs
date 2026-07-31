//! Git worktree isolation.
//!
//! # Why the Git CLI rather than libgit2
//!
//! `git worktree add/list/remove/prune` are stable and complete, whereas git2-rs's worktree
//! support is thin and blocking-only (so every call would need `spawn_blocking` anyway). More
//! decisively, libgit2 drags in libssh2/OpenSSL, which means code-signing and notarization pain
//! inside a Tauri bundle, and it would not honour the user's credential helpers, hooks or
//! `includeIf` config — all of which a real repository depends on. We already spawn `claude`,
//! so process spawning is a solved primitive here.
//!
//! # Serialization
//!
//! Every git command for a given repository goes through one mutex. `.git/index.lock`
//! contention is real once several agents are each running `git status`, and `worktree add`
//! takes that lock too. This is not defensive programming; it is the observed failure.

pub mod integration;
pub mod worktree;

pub use integration::{Contribution, IntegrationOutcome};
pub use worktree::{WorktreeManager, WorktreeSpec, WorktreeStatus};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("{path} is not inside a git repository")]
    NotARepository { path: PathBuf },
    #[error("git {command} failed: {stderr}")]
    CommandFailed { command: String, stderr: String },
    #[error(
        "worktree at {path} has uncommitted changes; refusing to remove it because that would \
         discard the only copy of the agent's work"
    )]
    DirtyWorktree { path: PathBuf },
    #[error("no worktree registered at {path}")]
    UnknownWorktree { path: PathBuf },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, GitError>;

/// Hands out one lock per repository path.
#[derive(Debug, Default)]
pub struct RepoLocks {
    locks: std::sync::Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl RepoLocks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_repo(&self, repo: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().expect("repo lock registry poisoned");
        locks.entry(repo.to_path_buf()).or_default().clone()
    }
}

/// Runs a git command in `cwd`. Callers must already hold the repo lock.
pub(crate) async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        // Keep git non-interactive: a credential or editor prompt would hang an agent forever
        // with no visible cause.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
        .await?;

    if !output.status.success() {
        return Err(GitError::CommandFailed {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolves the top level of the repository containing `path`.
pub async fn repo_root(path: &Path) -> Result<PathBuf> {
    let out = git(path, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|_| GitError::NotARepository {
            path: path.to_path_buf(),
        })?;

    let root = PathBuf::from(out);
    // Canonicalized so later `starts_with` containment checks compare like with like; on macOS
    // /var vs /private/var would otherwise never match.
    Ok(root.canonicalize().unwrap_or(root))
}
