//! The rows everything else hangs off: a workspace, a project, and the agents on its team.
//!
//! These exist because the schema enforces referential integrity, and a session with no agent
//! and no project is not a session anyone can reason about later — you could not say whose work
//! it was or which repository it touched. Persisting them is what makes every other durable
//! record answerable rather than a bag of ids.

use super::{Store, StoreError};
use crate::domain::ids::AgentId;
use std::path::{Path, PathBuf};

/// The local workspace and the project the app is currently pointed at.
#[derive(Debug, Clone)]
pub struct LocalIdentity {
    pub workspace_id: String,
    pub project_id: String,
}

/// The single-user workspace. There is exactly one until multi-workspace support exists, and a
/// fixed id rather than a generated one is what makes this idempotent across launches.
const LOCAL_WORKSPACE_ID: &str = "local";

/// Ensures the workspace and the project for `repo` exist, and returns their ids.
///
/// Keyed on the repository path, so reopening the same repository resumes the same project
/// rather than accumulating a new one per launch.
pub async fn ensure_project(store: &Store, repo: &Path) -> Result<LocalIdentity, StoreError> {
    let now = now_ms();
    let path = repo.display().to_string();

    sqlx::query(
        "INSERT INTO workspaces (id, name, created_at) VALUES (?1, 'Local', ?2)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(LOCAL_WORKSPACE_ID)
    .bind(now)
    .execute(store.writer())
    .await?;

    let name = repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());

    sqlx::query(
        "INSERT INTO projects (id, workspace_id, name, path, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (workspace_id, path) DO NOTHING",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(LOCAL_WORKSPACE_ID)
    .bind(&name)
    .bind(&path)
    .bind(now)
    .execute(store.writer())
    .await?;

    let (project_id,): (String,) =
        sqlx::query_as("SELECT id FROM projects WHERE workspace_id = ?1 AND path = ?2")
            .bind(LOCAL_WORKSPACE_ID)
            .bind(&path)
            .fetch_one(store.reader())
            .await?;

    Ok(LocalIdentity {
        workspace_id: LOCAL_WORKSPACE_ID.to_string(),
        project_id,
    })
}

/// Registers a team member for this run.
///
/// The slug is per-role and unique per workspace, so relaunching reuses the same agent row and
/// its history accumulates instead of resetting. The caller's `agent_id` wins on the first
/// insert; afterwards the stored id is authoritative and returned, which is what keeps a run's
/// events attributable to the same agent across restarts.
pub async fn ensure_agent(
    store: &Store,
    identity: &LocalIdentity,
    agent_id: AgentId,
    role: &str,
) -> Result<AgentId, StoreError> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO agents (id, workspace_id, project_id, name, slug, role, is_seeded,
                             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)
         ON CONFLICT (workspace_id, slug) DO NOTHING",
    )
    .bind(agent_id.to_string())
    .bind(&identity.workspace_id)
    .bind(&identity.project_id)
    .bind(title_case(role))
    .bind(role)
    .bind(role)
    .bind(now)
    .execute(store.writer())
    .await?;

    let (stored,): (String,) =
        sqlx::query_as("SELECT id FROM agents WHERE workspace_id = ?1 AND slug = ?2")
            .bind(&identity.workspace_id)
            .bind(role)
            .fetch_one(store.reader())
            .await?;

    Ok(stored
        .parse::<uuid::Uuid>()
        .map(AgentId::from)
        .unwrap_or(agent_id))
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Remembers which repository the operator chose.
///
/// Stored on the workspace rather than inferred from the process, because a `.app` opened from
/// Finder has a working directory of `/` — the choice has to survive a launch that carries no
/// context at all.
pub async fn set_active_project(store: &Store, path: &Path) -> Result<(), StoreError> {
    let settings = serde_json::json!({ "project_path": path.display().to_string() });
    sqlx::query("UPDATE workspaces SET settings_json = ?1 WHERE id = ?2")
        .bind(settings.to_string())
        .bind(LOCAL_WORKSPACE_ID)
        .execute(store.writer())
        .await?;
    Ok(())
}

/// The repository chosen last, if it is still a repository.
///
/// Re-validated on every read rather than trusted: a remembered path can be deleted, renamed, or
/// stop being a git repo between launches, and handing agents a directory that no longer exists
/// would fail far from the cause.
pub async fn active_project(store: &Store) -> Result<Option<PathBuf>, StoreError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT settings_json FROM workspaces WHERE id = ?1")
            .bind(LOCAL_WORKSPACE_ID)
            .fetch_optional(store.reader())
            .await?;

    let Some((settings,)) = row else {
        return Ok(None);
    };
    let parsed: serde_json::Value = serde_json::from_str(&settings).unwrap_or_default();
    let Some(raw) = parsed.get("project_path").and_then(|p| p.as_str()) else {
        return Ok(None);
    };
    let path = PathBuf::from(raw);
    Ok(is_repository(&path).then_some(path))
}

/// Whether a directory is inside a git repository, walking upward from it.
pub fn is_repository(path: &Path) -> bool {
    path.ancestors().any(|dir| dir.join(".git").exists())
}

/// The repository containing `path`, if any.
pub fn repository_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(|dir| dir.to_path_buf())
}
