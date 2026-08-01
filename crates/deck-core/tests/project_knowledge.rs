//! Standing project notes and skills, and the fact that editing one replaces it.
//!
//! These rows are read once at run start and rendered straight into every agent's system prompt,
//! so a duplicate or a stale row is not a display bug — it is contradictory instructions given to
//! workers that cannot see each other's transcripts.

use deck_core::store::knowledge::{self, Skill};
use deck_core::store::{identity, Store};
use std::path::PathBuf;

async fn project() -> (Store, String) {
    let store = Store::open_in_memory().await.expect("in-memory store");
    let id = identity::ensure_project(&store, &PathBuf::from("/tmp/knowledge-repo"))
        .await
        .unwrap()
        .project_id;
    (store, id)
}

fn skill(name: &str, body: &str) -> Skill {
    Skill {
        id: String::new(),
        name: name.into(),
        description: "when it applies".into(),
        body: body.into(),
        enabled: true,
    }
}

#[tokio::test]
async fn memory_round_trips_and_overwrites() {
    let (store, project_id) = project().await;

    assert_eq!(
        knowledge::memory(&store, &project_id).await.unwrap(),
        "",
        "a project with nothing recorded reads as empty, not as an error"
    );

    knowledge::save_memory(&store, &project_id, "migrations are hand-written")
        .await
        .unwrap();
    knowledge::save_memory(&store, &project_id, "and never generated")
        .await
        .unwrap();

    assert_eq!(
        knowledge::memory(&store, &project_id).await.unwrap(),
        "and never generated",
        "memory is one document, so saving replaces it rather than appending a second row"
    );
}

#[tokio::test]
async fn editing_a_skill_replaces_it_rather_than_adding_a_second() {
    // The failure this guards against is silent: two rows with the same name would both be
    // rendered into the prompt, so agents would be handed the old procedure and the new one at
    // once with nothing to say which won.
    let (store, project_id) = project().await;

    knowledge::save_skill(
        &store,
        &project_id,
        &skill("migrations", "write SQL by hand"),
    )
    .await
    .unwrap();
    knowledge::save_skill(
        &store,
        &project_id,
        &skill("migrations", "number them sequentially"),
    )
    .await
    .unwrap();

    let all = knowledge::skills(&store, &project_id).await.unwrap();
    assert_eq!(all.len(), 1, "same name means the same skill");
    assert_eq!(all[0].body, "number them sequentially");
}

#[tokio::test]
async fn a_deleted_skill_stops_reaching_agents() {
    let (store, project_id) = project().await;
    let id = knowledge::save_skill(&store, &project_id, &skill("styling", "use the tokens"))
        .await
        .unwrap();

    assert!(knowledge::delete_skill(&store, &id).await.unwrap());
    assert!(knowledge::skills(&store, &project_id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        knowledge::render_for_prompt("", &knowledge::skills(&store, &project_id).await.unwrap()),
        None
    );
}

#[test]
fn a_repository_with_claude_md_and_skills_is_discovered() {
    // The files a well-documented repository already has. Agents cannot read them — they spawn
    // with `--setting-sources ''` — so finding them is the whole point of the import.
    let dir = std::env::temp_dir().join(format!("deck-discover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".claude/skills/migrations")).unwrap();

    std::fs::write(dir.join("CLAUDE.md"), "Migrations are hand-written.\n").unwrap();
    std::fs::write(
        dir.join(".claude/skills/migrations/SKILL.md"),
        "---\nname: writing-migrations\ndescription: when changing the schema\n---\n\nNumber them sequentially.\n",
    )
    .unwrap();

    let found = knowledge::discover(&dir);
    assert_eq!(
        found.memory.as_deref(),
        Some("Migrations are hand-written.")
    );
    assert_eq!(found.skills.len(), 1);
    assert_eq!(found.skills[0].name, "writing-migrations");
    assert_eq!(found.skills[0].description, "when changing the schema");
    assert_eq!(found.skills[0].body, "Number them sequentially.");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_repository_with_nothing_documented_discovers_nothing() {
    let dir = std::env::temp_dir().join(format!("deck-discover-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    assert_eq!(knowledge::discover(&dir), knowledge::Discovered::default());

    let _ = std::fs::remove_dir_all(&dir);
}
