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

/// A repository registered in this workspace.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub path: String,
    /// False once the directory has been moved or deleted. Shown rather than hidden: a project
    /// vanishing from the list without explanation is more alarming than one marked missing.
    pub exists: bool,
}

/// Every repository the operator has opened in this workspace.
pub async fn list_projects(store: &Store) -> Result<Vec<ProjectRow>, StoreError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, name, path FROM projects WHERE workspace_id = ?1 ORDER BY created_at",
    )
    .bind(LOCAL_WORKSPACE_ID)
    .fetch_all(store.reader())
    .await?;

    Ok(rows
        .into_iter()
        // The placeholder row that exists only so the schema's foreign keys resolve when no
        // project has been chosen is not a project anyone opened.
        .filter(|(_, _, path)| path != "/nonexistent")
        .map(|(id, name, path)| ProjectRow {
            exists: is_repository(Path::new(&path)),
            id,
            name,
            path,
        })
        .collect())
}

/// Whether a repository has any commits yet.
///
/// Worth its own question: `git worktree add` needs something to branch from, and a repository
/// created a moment ago has no HEAD. An empty repo looks valid to every other check and then
/// fails the first time an agent is dispatched.
pub async fn has_commits(repo: &Path) -> bool {
    tokio::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(repo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Turns a plain directory into a repository agents can work in.
///
/// Two steps, and the second is not optional. `git init` alone leaves a repository with no HEAD,
/// and `git worktree add` has nothing to branch from — so the first dispatch would fail with an
/// error about an invalid reference, a long way from this decision. The empty commit gives every
/// worktree a base.
pub async fn initialize_repository(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }

    if repository_root(path).is_none() {
        run_git(path, &["init"]).await?;
    }

    if !has_commits(path).await {
        run_git(path, &["commit", "--allow-empty", "-m", "Initial commit"]).await?;
    }
    Ok(())
}

async fn run_git(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        // Never prompt: a credential or editor prompt here would hang with no visible cause.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
        .await
        .map_err(|e| format!("could not run git {}: {e}", args.join(" ")))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
