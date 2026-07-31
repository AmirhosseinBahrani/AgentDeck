//! Which agent processes exist, and which app launch owns them.
//!
//! Agents are process groups that outlive the app on Unix. If AgentDeck crashes mid-run, its
//! `claude` processes keep running, keep spending the account's usage allowance, and keep
//! writing into worktrees that the next launch believes are idle. Nothing in the app's own
//! memory survives to clean them up, so the record has to be on disk and has to be written
//! before there is anything to clean up.
//!
//! The hard part is not finding leftovers but deciding whether a leftover is ours to kill.
//! A second AgentDeck launched while the first is still running would otherwise see a table
//! full of live agents and reap all of them. Every row therefore carries the boot id and pid
//! of the app that spawned it, and reaping only touches rows whose owner is provably gone.

use super::{Store, StoreError};
use crate::process::{pid_is_alive, reap_orphan_group, Reaped};
use std::path::PathBuf;

/// Identifies one launch of the app.
///
/// Random per launch rather than derived from the pid: pids are reused, and a reused pid would
/// make a dead owner look alive and leave a real orphan running forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootId(pub String);

impl BootId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for BootId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for BootId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct ProcessRecord {
    pub session_id: String,
    pub task_id: Option<String>,
    pub pid: u32,
    pub pgid: u32,
    pub worktree_path: Option<PathBuf>,
}

/// Records a live agent. Called immediately after spawn, before the agent is handed any work.
pub async fn record(
    store: &Store,
    boot: &BootId,
    record: &ProcessRecord,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO runtime_processes
             (session_id, task_id, pid, pgid, started_at_ms, worktree_path, app_boot_id, app_pid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (session_id) DO UPDATE SET
             pid = excluded.pid, pgid = excluded.pgid, app_boot_id = excluded.app_boot_id,
             app_pid = excluded.app_pid",
    )
    .bind(&record.session_id)
    .bind(&record.task_id)
    .bind(record.pid as i64)
    .bind(record.pgid as i64)
    .bind(now_ms())
    .bind(
        record
            .worktree_path
            .as_ref()
            .map(|p| p.display().to_string()),
    )
    .bind(&boot.0)
    .bind(std::process::id() as i64)
    .execute(store.writer())
    .await?;
    Ok(())
}

/// Forgets a process that has exited or been killed.
pub async fn forget(store: &Store, session_id: &str) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM runtime_processes WHERE session_id = ?1")
        .bind(session_id)
        .execute(store.writer())
        .await?;
    Ok(())
}

/// What a boot-time reap did, so the app can tell the operator rather than doing it silently.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReapReport {
    /// Agents that survived a crash and have now been killed.
    pub killed: Vec<String>,
    /// Records for processes that were already gone.
    pub stale: Vec<String>,
    /// Left running because another app instance still owns them.
    pub owned_elsewhere: Vec<String>,
}

impl ReapReport {
    pub fn is_empty(&self) -> bool {
        self.killed.is_empty() && self.stale.is_empty() && self.owned_elsewhere.is_empty()
    }
}

/// Kills agents left behind by a previous launch and clears their records.
///
/// Runs before anything else touches a worktree. Two agents writing into one worktree would
/// interleave their commits and neither diff would be reviewable, so an orphan is not merely
/// wasted spend — it is a correctness problem for the run that is about to start.
pub async fn reap_orphans(store: &Store, boot: &BootId) -> Result<ReapReport, StoreError> {
    let rows: Vec<(String, i64, i64, String)> = sqlx::query_as(
        "SELECT session_id, pgid, app_pid, app_boot_id FROM runtime_processes WHERE app_boot_id != ?1",
    )
    .bind(&boot.0)
    .fetch_all(store.reader())
    .await?;

    let mut report = ReapReport::default();
    for (session_id, pgid, app_pid, _) in rows {
        // A live owner means a second instance is running this agent right now. Killing it
        // would destroy someone else's work, and the record is not ours to delete either.
        if pid_is_alive(app_pid as u32) {
            report.owned_elsewhere.push(session_id);
            continue;
        }

        match reap_orphan_group(pgid as u32) {
            Ok(Reaped::Killed) => report.killed.push(session_id.clone()),
            Ok(Reaped::AlreadyGone) => report.stale.push(session_id.clone()),
            Err(e) => {
                // Leave the row in place: a group we could not kill is one we must not forget,
                // or the next launch would have no record that it is still out there.
                tracing::error!(%session_id, %e, "could not reap an orphaned agent");
                continue;
            }
        }
        forget(store, &session_id).await?;
    }
    Ok(report)
}

/// One row as SQLite returns it, before it becomes a `ProcessRecord`.
type ProcessRow = (String, Option<String>, i64, i64, Option<String>);

/// Every process this launch believes is running. Used by shutdown to leave nothing behind.
pub async fn owned_by(store: &Store, boot: &BootId) -> Result<Vec<ProcessRecord>, StoreError> {
    let rows: Vec<ProcessRow> = sqlx::query_as(
        "SELECT session_id, task_id, pid, pgid, worktree_path
         FROM runtime_processes WHERE app_boot_id = ?1",
    )
    .bind(&boot.0)
    .fetch_all(store.reader())
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(session_id, task_id, pid, pgid, worktree_path)| ProcessRecord {
                session_id,
                task_id,
                pid: pid as u32,
                pgid: pgid as u32,
                worktree_path: worktree_path.map(PathBuf::from),
            },
        )
        .collect())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
