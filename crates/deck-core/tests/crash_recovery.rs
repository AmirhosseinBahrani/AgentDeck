//! What a restart does about the previous launch.
//!
//! The scenario these are written against: the app dies while agents are running. On Unix the
//! agents survive, because a process group outlives the process that created it. They keep
//! spending the account's usage allowance and keep writing into worktrees the next launch will
//! hand to somebody else. So the tests are about killing exactly the right things — and, just
//! as importantly, not killing a second instance's agents.

use deck_core::domain::event::ExitReason;
use deck_core::domain::ids::{AgentId, SessionId, TaskId};
use deck_core::store::identity;
use deck_core::store::processes::{self, BootId, ProcessRecord};
use deck_core::store::sessions::{self, NewSession};
use deck_core::store::Store;
use std::path::PathBuf;

/// A scratch directory that stands in for a worktree. Named per test rather than pooled, so a
/// test that removes its directory cannot make another one look unresumable.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("agentdeck-cr-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn remove(self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

fn record(session_id: &SessionId, pgid: u32) -> ProcessRecord {
    ProcessRecord {
        session_id: session_id.to_string(),
        task_id: None,
        pid: pgid,
        pgid,
        worktree_path: Some(PathBuf::from("/tmp/wt")),
    }
}

/// A process group that really is running, so liveness checks are not being fooled by a pid
/// that happens to be free.
async fn sleeping_group() -> tokio::process::Child {
    let mut cmd = tokio::process::Command::new("sleep");
    cmd.arg("30").kill_on_drop(true);
    deck_core::process::configure_group(&mut cmd);
    cmd.spawn().expect("spawn sleep")
}

#[tokio::test]
async fn an_agent_that_outlived_a_crash_is_killed() {
    let s = store().await;
    let previous = BootId::new();
    let mut child = sleeping_group().await;
    let pgid = child.id().expect("pid");

    processes::record(&s, &previous, &record(&SessionId::new(), pgid))
        .await
        .unwrap();

    // A pid that cannot exist stands in for the app that died. Using our own pid would make the
    // owner look alive and the orphan would correctly be left alone.
    sqlx::query("UPDATE runtime_processes SET app_pid = 0")
        .execute(s.writer())
        .await
        .unwrap();

    let report = processes::reap_orphans(&s, &BootId::new()).await.unwrap();
    assert_eq!(
        report.killed.len(),
        1,
        "the survivor should have been killed"
    );

    // The child is gone rather than merely signalled.
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .expect("the orphan should have died")
        .unwrap();
    assert!(!status.success(), "SIGKILL is not a clean exit");
}

#[tokio::test]
async fn a_second_instances_agents_are_left_running() {
    // The failure this prevents is the worst one available: launching a second window and
    // silently killing the first window's team.
    let s = store().await;
    let other = BootId::new();
    let mut child = sleeping_group().await;
    let pgid = child.id().expect("pid");

    // Owned by *this* process, which is alive — exactly what a concurrent instance looks like.
    processes::record(&s, &other, &record(&SessionId::new(), pgid))
        .await
        .unwrap();

    let report = processes::reap_orphans(&s, &BootId::new()).await.unwrap();
    assert!(
        report.killed.is_empty(),
        "must not kill a live owner's agent"
    );
    assert_eq!(report.owned_elsewhere.len(), 1);

    assert!(
        deck_core::process::pid_is_alive(pgid),
        "the other instance's agent should still be running"
    );
    let _ = child.kill().await;

    // And the record survives, because it is not ours to delete.
    let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM runtime_processes")
        .fetch_one(s.reader())
        .await
        .unwrap();
    assert_eq!(remaining.0, 1);
}

#[tokio::test]
async fn a_record_for_a_process_that_already_died_is_just_cleared() {
    let s = store().await;
    let previous = BootId::new();
    let mut child = sleeping_group().await;
    let pgid = child.id().expect("pid");
    child.kill().await.unwrap();
    child.wait().await.unwrap();

    processes::record(&s, &previous, &record(&SessionId::new(), pgid))
        .await
        .unwrap();
    sqlx::query("UPDATE runtime_processes SET app_pid = 0")
        .execute(s.writer())
        .await
        .unwrap();

    let report = processes::reap_orphans(&s, &BootId::new()).await.unwrap();
    assert_eq!(report.stale.len(), 1);
    assert!(report.killed.is_empty(), "nothing was alive to kill");

    let remaining: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM runtime_processes")
        .fetch_one(s.reader())
        .await
        .unwrap();
    assert_eq!(remaining.0, 0, "a stale record must not survive the reap");
}

#[tokio::test]
async fn this_launchs_own_agents_are_never_reaped() {
    // Reconcile runs at startup, but nothing stops it running again. It must be safe.
    let s = store().await;
    let boot = BootId::new();
    processes::record(&s, &boot, &record(&SessionId::new(), 424242))
        .await
        .unwrap();

    let report = processes::reap_orphans(&s, &boot).await.unwrap();
    assert!(report.is_empty(), "our own boot's rows are not orphans");
    assert_eq!(processes::owned_by(&s, &boot).await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_finished_agent_leaves_no_record_behind() {
    // A pid is reused within hours on a busy machine, so a record that outlives its process is
    // a loaded gun pointed at whatever inherits the number.
    let s = store().await;
    let boot = BootId::new();
    let session = SessionId::new();
    processes::record(&s, &boot, &record(&session, 999_999))
        .await
        .unwrap();

    processes::forget(&s, &session.to_string()).await.unwrap();
    assert!(processes::owned_by(&s, &boot).await.unwrap().is_empty());
}

// --- session persistence ------------------------------------------------------------------

async fn seeded_session(s: &Store, cwd: PathBuf) -> (SessionId, TaskId) {
    let identity = identity::ensure_project(s, &PathBuf::from("/tmp/repo"))
        .await
        .unwrap();
    let agent = identity::ensure_agent(s, &identity, AgentId::new(), "developer")
        .await
        .unwrap();

    let session_id = SessionId::new();
    let task_id = TaskId::new();
    sessions::record_started(
        s,
        &NewSession {
            session_id,
            agent_id: agent,
            project_id: identity.project_id,
            task_id: Some(task_id),
            cwd,
            argv: vec!["-p".into(), "--verbose".into()],
            model: None,
            permission_mode: "acceptEdits".into(),
        },
    )
    .await
    .unwrap();
    (session_id, task_id)
}

#[tokio::test]
async fn a_session_interrupted_by_a_crash_is_resumable_from_its_original_directory() {
    // The M0 spike found that --resume only works from the directory the session ran in, so
    // the directory is the whole point of persisting the row.
    let dir = Scratch::new("resume");
    let s = store().await;
    let (session_id, task_id) = seeded_session(&s, dir.path().to_path_buf()).await;

    // No record_ended: the app died.
    assert_eq!(sessions::mark_interrupted_on_boot(&s).await.unwrap(), 1);

    let resumable = sessions::resumable(&s, 10).await.unwrap();
    assert_eq!(resumable.len(), 1);
    assert_eq!(resumable[0].session_id, session_id);
    assert_eq!(resumable[0].task_id, Some(task_id));
    assert_eq!(resumable[0].cwd, dir.path());
    assert_eq!(resumable[0].status, "interrupted");
    assert!(resumable[0].cwd_exists);
}

#[tokio::test]
async fn a_session_whose_worktree_is_gone_is_reported_unresumable() {
    // Removing a worktree destroys the conversation with it. Saying so up front beats a resume
    // that fails with the CLI's own "no conversation found", which reads like a bug.
    let dir = Scratch::new("gone");
    let s = store().await;
    seeded_session(&s, dir.path().to_path_buf()).await;
    sessions::mark_interrupted_on_boot(&s).await.unwrap();
    dir.remove();

    let resumable = sessions::resumable(&s, 10).await.unwrap();
    assert_eq!(resumable.len(), 1);
    assert!(!resumable[0].cwd_exists);
}

#[tokio::test]
async fn a_session_the_operator_killed_is_not_offered_as_resumable() {
    // Killed is user intent. Offering it back would invite re-running work someone deliberately
    // stopped, and elsewhere the same distinction is what stops a retry being consumed.
    let dir = Scratch::new("killed");
    let s = store().await;
    let (session_id, _) = seeded_session(&s, dir.path().to_path_buf()).await;

    sessions::record_ended(&s, session_id, &ExitReason::Killed)
        .await
        .unwrap();
    sessions::mark_interrupted_on_boot(&s).await.unwrap();

    assert!(sessions::resumable(&s, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_crashed_session_is_offered_and_a_clean_one_is_not() {
    let dir = Scratch::new("crashed");
    let s = store().await;
    let (crashed, _) = seeded_session(&s, dir.path().to_path_buf()).await;
    let (clean, _) = seeded_session(&s, dir.path().to_path_buf()).await;

    sessions::record_ended(&s, crashed, &ExitReason::Crashed { code: Some(1) })
        .await
        .unwrap();
    sessions::record_ended(&s, clean, &ExitReason::Clean)
        .await
        .unwrap();

    let resumable = sessions::resumable(&s, 10).await.unwrap();
    assert_eq!(resumable.len(), 1);
    assert_eq!(resumable[0].session_id, crashed);
}

#[tokio::test]
async fn relaunching_reuses_the_same_project_and_agent_rows() {
    // An agent id that changed per launch would scatter one agent's history across a row per
    // run, and nothing downstream could answer "what has the reviewer done".
    let s = store().await;
    let repo = PathBuf::from("/tmp/repo");

    let first = identity::ensure_project(&s, &repo).await.unwrap();
    let first_agent = identity::ensure_agent(&s, &first, AgentId::new(), "reviewer")
        .await
        .unwrap();

    let second = identity::ensure_project(&s, &repo).await.unwrap();
    let second_agent = identity::ensure_agent(&s, &second, AgentId::new(), "reviewer")
        .await
        .unwrap();

    assert_eq!(first.project_id, second.project_id);
    assert_eq!(
        first_agent, second_agent,
        "the stored id must win over a freshly minted one"
    );

    let projects: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM projects")
        .fetch_one(s.reader())
        .await
        .unwrap();
    assert_eq!(projects.0, 1);
}
