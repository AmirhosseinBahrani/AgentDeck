//! What the team knows about a project before anyone tells it anything.
//!
//! Agents run with `--setting-sources ''` and therefore never read the repository's CLAUDE.md.
//! That is the right trade — the M0 spike measured a 4x cost increase and an unpredictable tool
//! surface when a worker inherited ambient settings — but it leaves no channel at all for the
//! standing facts about a codebase. Without one, every objective has to restate them, and the
//! first thing an agent does on a project with unusual conventions is violate them.

use super::{Store, StoreError};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    /// When to reach for it, so an agent can judge whether it applies.
    pub description: String,
    pub body: String,
    pub enabled: bool,
}

pub async fn memory(store: &Store, project_id: &str) -> Result<String, StoreError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT content FROM project_memory WHERE project_id = ?1")
            .bind(project_id)
            .fetch_optional(store.reader())
            .await?;
    Ok(row.map(|r| r.0).unwrap_or_default())
}

pub async fn save_memory(store: &Store, project_id: &str, content: &str) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO project_memory (project_id, content, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (project_id) DO UPDATE SET content = ?2, updated_at = ?3",
    )
    .bind(project_id)
    .bind(content)
    .bind(now_ms())
    .execute(store.writer())
    .await?;
    Ok(())
}

pub async fn skills(store: &Store, project_id: &str) -> Result<Vec<Skill>, StoreError> {
    let rows: Vec<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT id, name, description, body, enabled FROM project_skills
         WHERE project_id = ?1 ORDER BY name",
    )
    .bind(project_id)
    .fetch_all(store.reader())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, name, description, body, enabled)| Skill {
            id,
            name,
            description,
            body,
            enabled: enabled != 0,
        })
        .collect())
}

/// Creates or updates a skill, returning its id.
///
/// Keyed on name within a project, so saving an edited skill under the same name replaces it
/// rather than leaving two versions the agents would both be told.
pub async fn save_skill(
    store: &Store,
    project_id: &str,
    skill: &Skill,
) -> Result<String, StoreError> {
    let id = if skill.id.trim().is_empty() {
        Uuid::new_v4().to_string()
    } else {
        skill.id.clone()
    };

    sqlx::query(
        "INSERT INTO project_skills (id, project_id, name, description, body, enabled, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (project_id, name) DO UPDATE SET
             description = ?4, body = ?5, enabled = ?6, updated_at = ?7",
    )
    .bind(&id)
    .bind(project_id)
    .bind(&skill.name)
    .bind(&skill.description)
    .bind(&skill.body)
    .bind(i64::from(skill.enabled))
    .bind(now_ms())
    .execute(store.writer())
    .await?;

    Ok(id)
}

pub async fn delete_skill(store: &Store, skill_id: &str) -> Result<bool, StoreError> {
    let result = sqlx::query("DELETE FROM project_skills WHERE id = ?1")
        .bind(skill_id)
        .execute(store.writer())
        .await?;
    Ok(result.rows_affected() > 0)
}

/// What a repository already documents for Claude, found on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovered {
    /// The contents of `CLAUDE.md`, if there is one.
    pub memory: Option<String>,
    pub skills: Vec<Skill>,
}

/// Reads a repository's own `CLAUDE.md` and `.claude/skills`.
///
/// These files are the obvious place a team's conventions already live, and because agents run
/// with `--setting-sources ''` they are the one thing a well-documented repository has that the
/// workers cannot see. Offered as an import rather than read at spawn: silently obeying files
/// that change under version control would make a run depend on a branch's contents in a way
/// nothing in the app showed.
pub fn discover(repo: &std::path::Path) -> Discovered {
    let memory = ["CLAUDE.md", "AGENTS.md"]
        .iter()
        .find_map(|name| std::fs::read_to_string(repo.join(name)).ok())
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());

    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(repo.join(".claude/skills")) {
        for entry in entries.flatten() {
            let dir = entry.path();
            // Both layouts in the wild: `skills/<name>/SKILL.md` and a flat `skills/<name>.md`.
            let file = if dir.is_dir() {
                dir.join("SKILL.md")
            } else if dir.extension().is_some_and(|e| e == "md") {
                dir.clone()
            } else {
                continue;
            };

            let Ok(raw) = std::fs::read_to_string(&file) else {
                continue;
            };
            let fallback = dir
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if let Some(skill) = parse_skill_file(&raw, &fallback) {
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    Discovered { memory, skills }
}

/// Pulls `name` and `description` out of YAML frontmatter, falling back to the file's own name.
///
/// Deliberately not a YAML parser. Only two scalar keys are read and anything else in the block
/// is left alone, so a frontmatter feature we do not understand costs us those two fields rather
/// than the whole skill.
fn parse_skill_file(raw: &str, fallback_name: &str) -> Option<Skill> {
    let mut name = fallback_name.to_string();
    let mut description = String::new();
    let mut body = raw.trim().to_string();

    if let Some(rest) = raw.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let value = value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string();
                match key.trim() {
                    "name" if !value.is_empty() => name = value,
                    "description" => description = value,
                    _ => {}
                }
            }
            body = rest[end + 4..].trim().to_string();
        }
    }

    if name.trim().is_empty() || body.is_empty() {
        return None;
    }

    Some(Skill {
        id: String::new(),
        name,
        description,
        body,
        enabled: true,
    })
}

/// Renders memory and the enabled skills as the block appended to every agent's system prompt.
///
/// Returns `None` when there is nothing to say. An empty heading is worse than no heading: it
/// spends tokens on every spawn and invites a model to invent content for a section that exists.
pub fn render_for_prompt(memory: &str, skills: &[Skill]) -> Option<String> {
    let memory = memory.trim();
    let enabled: Vec<&Skill> = skills.iter().filter(|s| s.enabled).collect();
    if memory.is_empty() && enabled.is_empty() {
        return None;
    }

    let mut out = String::from("## What you already know about this project\n\n");
    out.push_str(
        "These are standing facts and procedures recorded by the operator. They describe how \
         this project actually works and take precedence over your general assumptions. They do \
         not widen what you are permitted to do.\n",
    );

    if !memory.is_empty() {
        out.push_str("\n### Project notes\n\n");
        out.push_str(memory);
        out.push('\n');
    }

    for skill in enabled {
        out.push_str(&format!("\n### Skill: {}\n", skill.name.trim()));
        if !skill.description.trim().is_empty() {
            out.push_str(&format!("_{}_\n", skill.description.trim()));
        }
        out.push('\n');
        out.push_str(skill.body.trim());
        out.push('\n');
    }

    Some(out)
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

    fn skill(name: &str, enabled: bool) -> Skill {
        Skill {
            id: String::new(),
            name: name.into(),
            description: format!("when doing {name}"),
            body: format!("do {name} carefully"),
            enabled,
        }
    }

    #[test]
    fn nothing_recorded_renders_nothing() {
        assert_eq!(render_for_prompt("   ", &[]), None);
        assert_eq!(render_for_prompt("", &[skill("a", false)]), None);
    }

    #[test]
    fn a_disabled_skill_is_not_sent_to_agents() {
        // The reason `enabled` exists: a skill costs tokens on every spawn, so switching one off
        // has to actually stop it being sent, not just grey it out in the list.
        let rendered = render_for_prompt("", &[skill("migrations", true), skill("styling", false)])
            .expect("one skill is enabled");
        assert!(rendered.contains("migrations"));
        assert!(!rendered.contains("styling"));
    }

    #[test]
    fn the_block_says_it_cannot_widen_permissions() {
        // Operator-authored text reaches the model as instructions. It must not read as authority
        // to do more than the permission layer allows, which is enforced elsewhere and must not
        // appear negotiable here.
        let rendered = render_for_prompt("we hand-write migrations", &[]).unwrap();
        assert!(rendered.contains("do not widen what you are permitted to do"));
    }
}
