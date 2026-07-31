//! Worktree manager tests, against real git repositories in temp directories.
//!
//! These never touch the AgentDeck repo itself: `git worktree prune` run against our own
//! checkout would delete sibling development worktrees.

use deck_core::git::worktree::{branch_name, WorktreeInfo};
use deck_core::git::{GitError, WorktreeManager, WorktreeSpec};
use std::path::{Path, PathBuf};

/// Real git invocations contend on `.git/index.lock`; serialize so failures are about the code.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn sh(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with one commit on `main`.
fn scratch_repo(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-wt-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();

    sh(&dir, &["init", "-q", "-b", "main"]);
    sh(&dir, &["config", "user.name", "Test"]);
    sh(&dir, &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.join("README.md"), "scratch\n").unwrap();
    sh(&dir, &["add", "README.md"]);
    sh(&dir, &["commit", "-q", "-m", "initial"]);
    dir
}

fn spec(title: &str) -> WorktreeSpec {
    WorktreeSpec {
        agent_slug: "backend".into(),
        task_short_id: "a1b2c3".into(),
        task_title: title.into(),
        base_ref: "main".into(),
    }
}

#[tokio::test]
async fn creates_an_isolated_worktree_on_its_own_branch() {
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("create");
    let m = WorktreeManager::new();

    let info = m.ensure(&repo, &spec("Implement JWT auth")).await.unwrap();

    assert!(info.path.exists(), "worktree directory should exist");
    assert!(
        info.path.starts_with(repo.join(".agentdeck")),
        "worktrees belong under .agentdeck, got {:?}",
        info.path
    );
    assert_eq!(info.branch, "agentdeck/backend/a1b2c3-implement-jwt-auth");
    assert!(!info.base_sha.is_empty());

    // The checkout is real and on the expected branch.
    assert!(info.path.join("README.md").exists());
    assert_eq!(
        sh(&info.path, &["rev-parse", "--abbrev-ref", "HEAD"]),
        info.branch
    );
}

#[tokio::test]
async fn the_worktree_directory_is_excluded_without_touching_a_tracked_file() {
    // Writing to .gitignore would be an uninvited change to the user's repository, and would
    // show up in their next commit.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("exclude");
    let m = WorktreeManager::new();
    m.ensure(&repo, &spec("task")).await.unwrap();

    let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.contains("/.agentdeck/"),
        "worktree dir should be in .git/info/exclude, got {exclude:?}"
    );
    assert!(
        !repo.join(".gitignore").exists(),
        ".gitignore must not be created"
    );

    // And the repository is clean: no stray untracked entries reported.
    assert_eq!(
        sh(&repo, &["status", "--porcelain"]),
        "",
        "creating a worktree must leave the repo clean"
    );
}

#[tokio::test]
async fn base_sha_is_pinned_so_a_review_diff_survives_main_moving() {
    // The reason base_sha is stored rather than a branch name: otherwise the diff a reviewer
    // sees would silently change as main advances.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("pin");
    let m = WorktreeManager::new();

    let info = m.ensure(&repo, &spec("pinned")).await.unwrap();
    let pinned = info.base_sha.clone();

    // Advance main after the worktree exists.
    std::fs::write(repo.join("other.txt"), "moved on\n").unwrap();
    sh(&repo, &["add", "other.txt"]);
    sh(&repo, &["commit", "-q", "-m", "advance main"]);
    let new_main = sh(&repo, &["rev-parse", "main"]);

    assert_ne!(new_main, pinned, "main should have moved");

    // The agent commits its own work.
    std::fs::write(info.path.join("feature.txt"), "agent work\n").unwrap();
    sh(&info.path, &["add", "feature.txt"]);
    sh(&info.path, &["commit", "-q", "-m", "agent change"]);

    let diff = m.diff(&repo, &info).await.unwrap();
    assert!(
        diff.contains("feature.txt"),
        "diff should include the agent's change"
    );
    assert!(
        !diff.contains("other.txt"),
        "diff must not include unrelated commits that landed on main afterwards: {diff}"
    );
}

#[tokio::test]
async fn status_reports_uncommitted_and_untracked_work_as_dirty() {
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("status");
    let m = WorktreeManager::new();
    let info = m.ensure(&repo, &spec("status")).await.unwrap();

    let clean = m.status(&repo, &info).await.unwrap();
    assert!(!clean.dirty, "a fresh worktree should be clean");
    assert_eq!(clean.commits_ahead_of_base, 0);

    // Untracked counts: an agent that created files without committing has still done work.
    std::fs::write(info.path.join("scratch.txt"), "wip\n").unwrap();
    let dirty = m.status(&repo, &info).await.unwrap();
    assert!(dirty.dirty, "untracked files must count as dirty");
    assert!(dirty
        .changed_files
        .iter()
        .any(|f| f.contains("scratch.txt")));
}

#[tokio::test]
async fn removing_a_dirty_worktree_is_refused() {
    // The worktree holds the only copy of uncommitted work, and this deletion is not
    // recoverable. Refusing is the whole point.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("dirty");
    let m = WorktreeManager::new();
    let info = m.ensure(&repo, &spec("dirty")).await.unwrap();

    std::fs::write(info.path.join("unsaved.txt"), "precious\n").unwrap();

    let err = m.remove(&repo, &info, false).await;
    assert!(
        matches!(err, Err(GitError::DirtyWorktree { .. })),
        "expected refusal, got {err:?}"
    );
    assert!(
        info.path.join("unsaved.txt").exists(),
        "the agent's work must still be there after a refused removal"
    );
}

#[tokio::test]
async fn a_clean_worktree_can_be_removed() {
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("clean");
    let m = WorktreeManager::new();
    let info = m.ensure(&repo, &spec("clean")).await.unwrap();

    m.remove(&repo, &info, false).await.expect("clean removal");
    assert!(!info.path.exists());

    let remaining = m.list(&repo).await.unwrap();
    assert!(
        !remaining.contains(&info.path),
        "git should no longer list the removed worktree"
    );
}

#[tokio::test]
async fn forcing_removal_is_possible_but_never_automatic() {
    // Force exists for an explicit operator decision. The signature requires opting in, so no
    // internal retry path can reach it by accident.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("force");
    let m = WorktreeManager::new();
    let info = m.ensure(&repo, &spec("force")).await.unwrap();
    std::fs::write(info.path.join("wip.txt"), "x\n").unwrap();

    assert!(m.remove(&repo, &info, false).await.is_err());
    m.remove(&repo, &info, true).await.expect("forced removal");
    assert!(!info.path.exists());
}

#[tokio::test]
async fn ensure_is_idempotent_so_crash_recovery_does_not_fail() {
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("idem");
    let m = WorktreeManager::new();

    let first = m.ensure(&repo, &spec("idem")).await.unwrap();
    // Simulates a retry or a reconcile pass after a crash.
    let second = m.ensure(&repo, &spec("idem")).await.unwrap();

    assert_eq!(first.path, second.path);
    assert_eq!(first.branch, second.branch);
}

#[tokio::test]
async fn two_agents_get_separate_trees_and_do_not_see_each_others_edits() {
    // The core isolation guarantee: parallel agents editing the same file must not conflict.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("parallel");
    let m = WorktreeManager::new();

    let backend = m
        .ensure(
            &repo,
            &WorktreeSpec {
                agent_slug: "backend".into(),
                task_short_id: "aaa111".into(),
                task_title: "backend work".into(),
                base_ref: "main".into(),
            },
        )
        .await
        .unwrap();

    let frontend = m
        .ensure(
            &repo,
            &WorktreeSpec {
                agent_slug: "frontend".into(),
                task_short_id: "bbb222".into(),
                task_title: "frontend work".into(),
                base_ref: "main".into(),
            },
        )
        .await
        .unwrap();

    assert_ne!(backend.path, frontend.path);
    assert_ne!(backend.branch, frontend.branch);

    // Both edit the same file, independently.
    std::fs::write(backend.path.join("README.md"), "backend version\n").unwrap();
    std::fs::write(frontend.path.join("README.md"), "frontend version\n").unwrap();

    assert_eq!(
        std::fs::read_to_string(backend.path.join("README.md")).unwrap(),
        "backend version\n"
    );
    assert_eq!(
        std::fs::read_to_string(frontend.path.join("README.md")).unwrap(),
        "frontend version\n"
    );
    // And the shared repo is untouched by either.
    assert_eq!(
        std::fs::read_to_string(repo.join("README.md")).unwrap(),
        "scratch\n"
    );
}

#[tokio::test]
async fn prune_reconciles_gits_records_with_the_filesystem() {
    // After a crash the database, git's records and the filesystem all disagree. Boot must
    // converge them rather than trusting any one of them.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("prune");
    let m = WorktreeManager::new();
    let info = m.ensure(&repo, &spec("prune")).await.unwrap();

    assert!(m.list(&repo).await.unwrap().contains(&info.path));

    // Delete the directory behind git's back, as a crash or an external cleanup would.
    std::fs::remove_dir_all(&info.path).unwrap();

    let after = m.prune_and_list(&repo).await.unwrap();
    assert!(
        !after.contains(&info.path),
        "prune should drop records for directories that no longer exist"
    );
}

#[tokio::test]
async fn status_of_a_missing_worktree_is_an_error_not_a_false_clean() {
    // Reporting "clean" for a vanished worktree would let a caller delete records for work
    // that might still be recoverable.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("missing");
    let m = WorktreeManager::new();

    let bogus = WorktreeInfo {
        path: repo.join(".agentdeck/agent-x/task-y"),
        branch: "agentdeck/x/y".into(),
        base_ref: "main".into(),
        base_sha: "deadbeef".into(),
    };

    assert!(matches!(
        m.status(&repo, &bogus).await,
        Err(GitError::UnknownWorktree { .. })
    ));
}

#[tokio::test]
async fn a_non_repository_path_is_reported_clearly() {
    let _s = SERIAL.lock().await;
    let dir = std::env::temp_dir().join(format!("agentdeck-norepo-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let m = WorktreeManager::new();

    assert!(matches!(
        m.ensure(&dir, &spec("x")).await,
        Err(GitError::NotARepository { .. })
    ));
}

#[test]
fn branch_names_are_valid_git_refs_even_from_hostile_titles() {
    // Task titles come from a model and can contain anything. An invalid ref would fail at
    // worktree creation, which is a confusing place to discover a bad title.
    let long = "a".repeat(200);
    let cases = [
        "Implement JWT auth",
        "Fix: the ~thing~ [broken]!",
        "  spaces   everywhere  ",
        "emoji 🚀 title",
        long.as_str(),
        "...",
        "",
    ];

    for title in cases {
        let branch = branch_name("dev", "abc123", title);
        assert!(!branch.contains(".."), "{branch} contains ..");
        assert!(!branch.ends_with('.'), "{branch} ends with .");
        assert!(!branch.ends_with('/'), "{branch} ends with /");
        assert!(!branch.contains("//"), "{branch} contains //");
        assert!(
            !branch.contains(|c: char| c.is_whitespace()),
            "{branch} contains whitespace"
        );
        for bad in ['~', '^', ':', '?', '*', '[', '\\'] {
            assert!(!branch.contains(bad), "{branch} contains {bad}");
        }
        assert!(branch.starts_with("agentdeck/dev/abc123"));
    }
}
