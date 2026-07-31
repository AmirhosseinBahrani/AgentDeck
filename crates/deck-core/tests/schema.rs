//! Schema invariants. These guard the constraints the orchestration logic relies on being
//! true at the database level rather than only in Rust — anything enforced only in code
//! will eventually be bypassed by a new call site.

use deck_core::store::Store;
use sqlx::Row;

async fn store() -> Store {
    Store::open_in_memory().await.expect("open in-memory store")
}

fn now() -> i64 {
    1_760_000_000_000
}

async fn seed_project(s: &Store) -> (String, String) {
    let ws = uuid::Uuid::new_v4().to_string();
    let proj = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO workspaces (id, name, created_at) VALUES (?, 'ws', ?)")
        .bind(&ws)
        .bind(now())
        .execute(s.writer())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO projects (id, workspace_id, name, path, created_at) VALUES (?, ?, 'p', ?, ?)",
    )
    .bind(&proj)
    .bind(&ws)
    .bind(format!("/tmp/{proj}"))
    .bind(now())
    .execute(s.writer())
    .await
    .unwrap();
    (ws, proj)
}

async fn seed_task(s: &Store, project: &str, status: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO tasks (id, project_id, title, status, created_at) VALUES (?, ?, 't', ?, ?)",
    )
    .bind(&id)
    .bind(project)
    .bind(status)
    .bind(now())
    .execute(s.writer())
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn migrations_apply_and_expected_tables_exist() {
    let s = store().await;
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(s.reader())
        .await
        .unwrap();
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("name")).collect();

    for expected in [
        "workspaces",
        "projects",
        "agents",
        "agent_runtime_states",
        "supervisor_runs",
        "tasks",
        "task_dependencies",
        "sessions",
        "messages",
        "tool_calls",
        "permission_requests",
        "worktrees",
        "mcp_servers",
        "agent_reports",
        "reviews",
        "events",
        // Beyond the spec's list, required by the design:
        "decisions",
        "stage_receipts",
        "intents",
        "resource_locks",
        "runtime_processes",
        "escalations",
        "rate_limit_state",
    ] {
        assert!(tables.contains(&expected.to_string()), "missing {expected}");
    }
}

#[tokio::test]
async fn foreign_keys_are_enforced_not_merely_declared() {
    // SQLite ignores foreign keys unless the pragma is on per-connection, which is easy
    // to get wrong and silently permits orphan rows.
    let s = store().await;
    let err = sqlx::query(
        "INSERT INTO projects (id, workspace_id, name, path, created_at)
         VALUES ('p1', 'nonexistent-workspace', 'p', '/tmp/x', 0)",
    )
    .execute(s.writer())
    .await;
    assert!(err.is_err(), "orphan project should be rejected");
}

#[tokio::test]
async fn task_status_is_constrained_to_the_known_state_machine() {
    let s = store().await;
    let (_, proj) = seed_project(&s).await;
    let bad = sqlx::query(
        "INSERT INTO tasks (id, project_id, title, status, created_at)
         VALUES ('t1', ?, 't', 'almost_done', 0)",
    )
    .bind(&proj)
    .execute(s.writer())
    .await;
    assert!(bad.is_err(), "invented task status should be rejected");
}

#[tokio::test]
async fn a_task_cannot_depend_on_itself() {
    let s = store().await;
    let (_, proj) = seed_project(&s).await;
    let t = seed_task(&s, &proj, "backlog").await;

    let self_dep =
        sqlx::query("INSERT INTO task_dependencies (task_id, depends_on_task_id) VALUES (?, ?)")
            .bind(&t)
            .bind(&t)
            .execute(s.writer())
            .await;
    assert!(self_dep.is_err(), "self-dependency must be rejected");
}

#[tokio::test]
async fn resource_locks_are_unique_per_key_and_task() {
    let s = store().await;
    let (_, proj) = seed_project(&s).await;
    let t = seed_task(&s, &proj, "running").await;

    let insert = |mode: &'static str| {
        let t = t.clone();
        let w = s.writer().clone();
        async move {
            sqlx::query(
                "INSERT INTO resource_locks (resource_key, mode, task_id, acquired_at)
                 VALUES ('worktree:/tmp/wt', ?, ?, 0)",
            )
            .bind(mode)
            .bind(&t)
            .execute(&w)
            .await
        }
    };

    insert("exclusive").await.unwrap();
    assert!(
        insert("exclusive").await.is_err(),
        "the same task must not double-acquire one resource key"
    );
}

#[tokio::test]
async fn events_seq_is_monotonic_and_survives_deletes() {
    // The UI detects gaps and backfills by seq, so ids must never be reused after pruning.
    let s = store().await;
    for kind in ["a", "b", "c"] {
        sqlx::query("INSERT INTO events (at, kind, payload_json) VALUES (0, ?, '{}')")
            .bind(kind)
            .execute(s.writer())
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM events WHERE kind = 'c'")
        .execute(s.writer())
        .await
        .unwrap();
    sqlx::query("INSERT INTO events (at, kind, payload_json) VALUES (0, 'd', '{}')")
        .execute(s.writer())
        .await
        .unwrap();

    let seqs: Vec<i64> = sqlx::query("SELECT seq FROM events ORDER BY seq")
        .fetch_all(s.reader())
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<i64, _>("seq"))
        .collect();

    assert_eq!(seqs, vec![1, 2, 4], "AUTOINCREMENT must not recycle seq 3");
}

#[tokio::test]
async fn only_one_row_of_global_rate_limit_state_can_exist() {
    let s = store().await;
    sqlx::query("INSERT INTO rate_limit_state (id, status, observed_at) VALUES (1, 'allowed', 0)")
        .execute(s.writer())
        .await
        .unwrap();
    let second =
        sqlx::query("INSERT INTO rate_limit_state (id, status, observed_at) VALUES (2, 'x', 0)")
            .execute(s.writer())
            .await;
    assert!(
        second.is_err(),
        "throttling is account-wide, so the state must be a singleton"
    );
}

#[tokio::test]
async fn sessions_require_a_cwd_because_resume_depends_on_it() {
    let s = store().await;
    let (ws, proj) = seed_project(&s).await;
    sqlx::query(
        "INSERT INTO agents (id, workspace_id, name, slug, role, created_at, updated_at)
         VALUES ('a1', ?, 'Dev', 'dev', 'developer', 0, 0)",
    )
    .bind(&ws)
    .execute(s.writer())
    .await
    .unwrap();

    let missing_cwd = sqlx::query(
        "INSERT INTO sessions (id, agent_id, project_id, status) VALUES ('s1', 'a1', ?, 'starting')",
    )
    .bind(&proj)
    .execute(s.writer())
    .await;
    assert!(
        missing_cwd.is_err(),
        "a session without cwd can never be resumed, so it must not be storable"
    );
}
