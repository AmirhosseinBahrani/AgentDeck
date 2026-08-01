//! The team, as something you can change.
//!
//! Roles were three seeded constants. The supervisor already assigns work by matching a task's
//! role against the agents that hold it, and the planner is told which roles exist — so a role
//! only had to become data for hiring to work end to end. A "Database Engineer" is assignable
//! the moment the row exists.
//!
//! Revoking deactivates rather than deletes. The run record's whole value is that you can read
//! what happened afterwards, and removing the row would take every session, report and decision
//! that referenced the agent with it.

use super::identity::LocalIdentity;
use super::{Store, StoreError};
use crate::domain::ids::AgentId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub name: String,
    pub slug: String,
    /// The capability the supervisor assigns against. Free-form on purpose.
    pub role: String,
    pub model: Option<String>,
    /// Appended to the CLI's system prompt for every session this agent runs.
    pub system_prompt: Option<String>,
    pub mcp_servers: Vec<String>,
    pub max_concurrent_sessions: u32,
    pub is_seeded: bool,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct NewAgent {
    pub name: String,
    pub role: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub mcp_servers: Vec<String>,
}

/// Everyone currently on the team.
pub async fn active(
    store: &Store,
    identity: &LocalIdentity,
) -> Result<Vec<AgentRecord>, StoreError> {
    load(store, identity, true).await
}

/// Everyone who has ever been on it, including revoked members.
pub async fn all(store: &Store, identity: &LocalIdentity) -> Result<Vec<AgentRecord>, StoreError> {
    load(store, identity, false).await
}

type Row = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    i64,
    i64,
);

async fn load(
    store: &Store,
    identity: &LocalIdentity,
    only_active: bool,
) -> Result<Vec<AgentRecord>, StoreError> {
    let sql = if only_active {
        "SELECT id, name, slug, role, model_json, system_prompt, mcp_server_ids_json,
                max_concurrent_sessions, is_seeded, active
         FROM agents WHERE workspace_id = ?1 AND active = 1 ORDER BY created_at"
    } else {
        "SELECT id, name, slug, role, model_json, system_prompt, mcp_server_ids_json,
                max_concurrent_sessions, is_seeded, active
         FROM agents WHERE workspace_id = ?1 ORDER BY created_at"
    };

    let rows: Vec<Row> = sqlx::query_as(sql)
        .bind(&identity.workspace_id)
        .fetch_all(store.reader())
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(AgentRecord {
                id: r.0.parse::<uuid::Uuid>().ok().map(AgentId::from)?,
                name: r.1,
                slug: r.2,
                role: r.3,
                // Stored as JSON for room to grow; a bare string is the common case.
                model: r.4.and_then(|m| serde_json::from_str::<String>(&m).ok()),
                system_prompt: r.5,
                mcp_servers: serde_json::from_str(&r.6).unwrap_or_default(),
                max_concurrent_sessions: r.7 as u32,
                is_seeded: r.8 != 0,
                active: r.9 != 0,
            })
        })
        .collect())
}

/// Adds someone to the team.
///
/// Reactivates rather than duplicating when the slug already exists: the design calls revoking
/// reversible and says a role can be rehired from its profile, so hiring the same role twice has
/// to return the original agent — otherwise its history would be orphaned behind a new id.
pub async fn hire(
    store: &Store,
    identity: &LocalIdentity,
    agent: &NewAgent,
) -> Result<AgentRecord, StoreError> {
    let slug = slugify(&agent.name);
    let now = now_ms();

    sqlx::query(
        "INSERT INTO agents (id, workspace_id, project_id, name, slug, role, model_json,
                             system_prompt, mcp_server_ids_json, is_seeded, active,
                             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 1, ?10, ?10)
         ON CONFLICT (workspace_id, slug) DO UPDATE SET
             active = 1, name = excluded.name, role = excluded.role,
             model_json = excluded.model_json, system_prompt = excluded.system_prompt,
             mcp_server_ids_json = excluded.mcp_server_ids_json, updated_at = excluded.updated_at",
    )
    .bind(AgentId::new().to_string())
    .bind(&identity.workspace_id)
    .bind(&identity.project_id)
    .bind(&agent.name)
    .bind(&slug)
    .bind(&agent.role)
    .bind(serde_json::to_string(&agent.model).unwrap_or_else(|_| "null".into()))
    .bind(&agent.system_prompt)
    .bind(serde_json::to_string(&agent.mcp_servers).unwrap_or_else(|_| "[]".into()))
    .bind(now)
    .execute(store.writer())
    .await?;

    active(store, identity)
        .await?
        .into_iter()
        .find(|a| a.slug == slug)
        .ok_or_else(|| StoreError::Sqlx(sqlx::Error::RowNotFound))
}

/// Takes someone off the roster.
///
/// Deactivates, never deletes — see the module note. Returns false for an id that is not on the
/// team, which is the ordinary result of a double click or a stale window.
pub async fn revoke(store: &Store, agent_id: AgentId) -> Result<bool, StoreError> {
    let result = sqlx::query("UPDATE agents SET active = 0, updated_at = ?1 WHERE id = ?2")
        .bind(now_ms())
        .bind(agent_id.to_string())
        .execute(store.writer())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// A slug that is stable across rehires, since it is what identifies the same seat on the team.
fn slugify(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse runs of separators so "Docs  Writer" and "Docs-Writer" are the same seat.
    let mut out = String::new();
    let mut last_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !last_dash && !out.is_empty() {
                out.push('-');
            }
            last_dash = true;
        } else {
            out.push(c);
            last_dash = false;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_a_stable_slug() {
        assert_eq!(slugify("Database Engineer"), "database-engineer");
        assert_eq!(slugify("Docs  Writer"), "docs-writer");
        assert_eq!(slugify("Docs-Writer"), "docs-writer");
        assert_eq!(slugify("  QA  "), "qa");
    }

    #[test]
    fn punctuation_does_not_produce_trailing_separators() {
        // The slug is a unique key, so "Backend!" and "Backend" must not become two seats.
        assert_eq!(slugify("Backend!"), "backend");
        assert_eq!(slugify("C++ Engineer"), "c-engineer");
    }
}

/// What one agent has actually done, aggregated across every run on this project.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentMetrics {
    pub agent_id: AgentId,
    pub name: String,
    pub role: String,
    pub sessions: i64,
    pub tasks_completed: i64,
    pub tasks_failed: i64,
    /// Attempts summed across the agent's tasks. Higher than the task count means rework.
    pub attempts: i64,
    pub review_rounds: i64,
    pub cost_usd: f64,
    pub turns: i64,
    pub last_active_ms: Option<i64>,
}

/// Per-agent totals for the whole project.
///
/// Two separate aggregates rather than one join. Summing session cost and task counts in a single
/// query multiplies one by the cardinality of the other, which silently inflates both — the
/// classic fan-out that makes a metrics page confidently wrong.
pub async fn metrics(
    store: &Store,
    identity: &LocalIdentity,
) -> Result<Vec<AgentMetrics>, StoreError> {
    let roster = active(store, identity).await?;

    let session_rows: Vec<(String, i64, f64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT agent_id, COUNT(*), COALESCE(SUM(cost_usd), 0), COALESCE(SUM(num_turns), 0),
                MAX(COALESCE(last_activity_at, ended_at, started_at))
         FROM sessions WHERE project_id = ?1 GROUP BY agent_id",
    )
    .bind(&identity.project_id)
    .fetch_all(store.reader())
    .await?;

    let task_rows: Vec<(String, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT assignee_agent_id,
                SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END),
                SUM(CASE WHEN status IN ('failed', 'cancelled') THEN 1 ELSE 0 END),
                COALESCE(SUM(attempts), 0), COALESCE(SUM(review_rounds), 0)
         FROM tasks WHERE project_id = ?1 AND assignee_agent_id IS NOT NULL
         GROUP BY assignee_agent_id",
    )
    .bind(&identity.project_id)
    .fetch_all(store.reader())
    .await?;

    Ok(roster
        .into_iter()
        .map(|agent| {
            let key = agent.id.to_string();
            let session = session_rows.iter().find(|r| r.0 == key);
            let task = task_rows.iter().find(|r| r.0 == key);

            AgentMetrics {
                agent_id: agent.id,
                name: agent.name,
                role: agent.role,
                sessions: session.map(|r| r.1).unwrap_or(0),
                cost_usd: session.map(|r| r.2).unwrap_or(0.0),
                turns: session.map(|r| r.3).unwrap_or(0),
                last_active_ms: session.and_then(|r| r.4),
                tasks_completed: task.map(|r| r.1).unwrap_or(0),
                tasks_failed: task.map(|r| r.2).unwrap_or(0),
                attempts: task.map(|r| r.3).unwrap_or(0),
                review_rounds: task.map(|r| r.4).unwrap_or(0),
            }
        })
        .collect())
}
