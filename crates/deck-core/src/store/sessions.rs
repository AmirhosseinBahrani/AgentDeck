//! Durable session records, kept so a session can be resumed after the app restarts.
//!
//! The M0 spike established the constraint this module exists to satisfy: Claude buckets
//! transcripts by working directory, and `--resume` from anywhere else fails with "no
//! conversation found". The directory an agent ran in is therefore not a diagnostic detail but
//! the only thing that makes its conversation reachable again — and nothing else records it,
//! since the worktree registry lives in memory and dies with the process.

use super::{Store, StoreError};
use crate::domain::event::ExitReason;
use crate::domain::ids::{AgentId, SessionId, TaskId};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct NewSession {
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub project_id: String,
    pub task_id: Option<TaskId>,
    pub cwd: PathBuf,
    pub argv: Vec<String>,
    pub model: Option<String>,
    pub permission_mode: String,
}

/// A session that could be resumed, with everything `--resume` needs to work.
#[derive(Debug, Clone, PartialEq)]
pub struct ResumableSession {
    pub session_id: SessionId,
    pub task_id: Option<TaskId>,
    pub cwd: PathBuf,
    /// Why it is not running: `interrupted` after a crash, `stopped` after a clean exit.
    pub status: String,
    /// False when the directory has since been removed, which makes the conversation
    /// unreachable no matter what we do. Surfaced rather than discovered at spawn time.
    pub cwd_exists: bool,
}

pub async fn record_started(store: &Store, session: &NewSession) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO sessions (id, agent_id, project_id, task_id, status, cwd, model,
                               permission_mode, argv_json, started_at, last_activity_at)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?7, ?8, ?9, ?9)
         ON CONFLICT (id) DO UPDATE SET status = 'running', started_at = excluded.started_at",
    )
    .bind(session.session_id.to_string())
    .bind(session.agent_id.to_string())
    .bind(&session.project_id)
    .bind(session.task_id.map(|t| t.to_string()))
    .bind(session.cwd.display().to_string())
    .bind(&session.model)
    .bind(&session.permission_mode)
    .bind(serde_json::to_string(&session.argv).unwrap_or_else(|_| "[]".into()))
    .bind(now_ms())
    .execute(store.writer())
    .await?;
    Ok(())
}

/// Closes out a session with why it ended.
///
/// The status distinction is load-bearing, not cosmetic: a user-killed session must never look
/// like a crash, because a crash is what makes the supervisor consume a retry and reassign work
/// the operator deliberately stopped.
pub async fn record_ended(
    store: &Store,
    session_id: SessionId,
    reason: &ExitReason,
) -> Result<(), StoreError> {
    let (status, code, detail) = match reason {
        ExitReason::Clean => ("stopped", None, None),
        ExitReason::Interrupted => ("interrupted", None, None),
        ExitReason::Killed => ("stopped", None, Some("killed by the operator".to_string())),
        ExitReason::Crashed { code } => ("crashed", *code, None),
        ExitReason::StartupFailed { detail } => ("failed", None, Some(detail.clone())),
    };

    sqlx::query(
        "UPDATE sessions SET status = ?1, exit_code = ?2, exit_reason = ?3, ended_at = ?4
         WHERE id = ?5",
    )
    .bind(status)
    .bind(code)
    .bind(detail)
    .bind(now_ms())
    .bind(session_id.to_string())
    .execute(store.writer())
    .await?;
    Ok(())
}

/// Marks sessions that a previous launch left mid-flight.
///
/// Anything still `running` when the app starts cannot actually be running — this launch has
/// spawned nothing yet. Leaving those rows alone would make the UI show live agents that do not
/// exist, and would make the resumable list omit exactly the sessions a crash interrupted.
pub async fn mark_interrupted_on_boot(store: &Store) -> Result<u64, StoreError> {
    let result = sqlx::query(
        "UPDATE sessions SET status = 'interrupted', ended_at = ?1
         WHERE status IN ('starting', 'running', 'idle')",
    )
    .bind(now_ms())
    .execute(store.writer())
    .await?;
    Ok(result.rows_affected())
}

/// Sessions that ended without completing, newest first.
pub async fn resumable(store: &Store, limit: i64) -> Result<Vec<ResumableSession>, StoreError> {
    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT id, task_id, cwd, status FROM sessions
         WHERE status IN ('interrupted', 'crashed')
         ORDER BY COALESCE(ended_at, started_at) DESC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(store.reader())
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(id, task_id, cwd, status)| {
            Some(ResumableSession {
                session_id: id.parse::<uuid::Uuid>().ok().map(SessionId::from)?,
                task_id: task_id
                    .and_then(|t| t.parse::<uuid::Uuid>().ok())
                    .map(TaskId::from),
                cwd_exists: Path::new(&cwd).is_dir(),
                cwd: PathBuf::from(cwd),
                status,
            })
        })
        .collect())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
