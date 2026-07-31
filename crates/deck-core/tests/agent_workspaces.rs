//! Per-agent containment.
//!
//! The guarantee under test: one agent's boundary is its own worktree, and never the union of
//! everyone's. A shared broker would still "work" in the happy path while quietly letting
//! agents write into each other's trees, so the tests are written to catch exactly that.

use deck_core::domain::ids::{AgentId, TaskId};
use deck_core::permission::{Decision, PolicyLayer, Resolution};
use deck_core::workspace::{PrepareRequest, WorkspaceRegistry};
use serde_json::json;
use std::path::{Path, PathBuf};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn sh(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scratch_repo(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-ws-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    sh(&dir, &["init", "-q", "-b", "main"]);
    sh(&dir, &["config", "user.name", "Test"]);
    sh(&dir, &["config", "user.email", "t@example.com"]);
    std::fs::write(dir.join("README.md"), "scratch\n").unwrap();
    sh(&dir, &["add", "README.md"]);
    sh(&dir, &["commit", "-q", "-m", "initial"]);
    dir
}

fn request<'a>(slug: &'a str, task: TaskId, title: &'a str) -> PrepareRequest<'a> {
    PrepareRequest {
        agent_id: AgentId::new(),
        agent_slug: slug,
        task_id: task,
        task_title: title,
        base_ref: "main",
        system_prompt: None,
        model: None,
        extra_layers: vec![],
    }
}

#[tokio::test]
async fn each_agent_is_confined_to_its_own_worktree() {
    // The core guarantee. With one shared broker this test would pass trivially in the happy
    // path while agents could write into each other's trees.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("confine");
    let reg = WorkspaceRegistry::new(repo.clone());

    let a_task = TaskId::new();
    let b_task = TaskId::new();
    let a = reg
        .prepare(request("backend", a_task, "backend work"))
        .await
        .unwrap();
    let b = reg
        .prepare(request("frontend", b_task, "frontend work"))
        .await
        .unwrap();

    assert_ne!(a.worktree_path(), b.worktree_path());

    // Each agent may write inside its own tree.
    let own = a.worktree_path().join("mine.rs");
    let (decision, _) = a
        .broker
        .policy()
        .decide("Write", &json!({ "file_path": own, "content": "x" }));
    assert!(
        matches!(decision, Decision::Allow { .. }),
        "an agent must be free inside its own worktree: {decision:?}"
    );

    // But not inside the other agent's tree.
    let theirs = b.worktree_path().join("theirs.rs");
    let (decision, _) = a
        .broker
        .policy()
        .decide("Write", &json!({ "file_path": theirs, "content": "x" }));
    assert!(
        !matches!(decision, Decision::Allow { .. }),
        "agent A wrote into agent B's worktree unchallenged: {decision:?}"
    );

    // Nor into the shared repository root.
    let shared = repo.join("README.md");
    let (decision, _) = a
        .broker
        .policy()
        .decide("Write", &json!({ "file_path": shared, "content": "x" }));
    assert!(
        !matches!(decision, Decision::Allow { .. }),
        "agent A modified the shared repo unchallenged: {decision:?}"
    );
}

#[tokio::test]
async fn brokers_are_independent_so_one_agents_prompt_does_not_appear_on_another() {
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("independent");
    let reg = WorkspaceRegistry::new(repo);

    let a = reg
        .prepare(request("backend", TaskId::new(), "a"))
        .await
        .unwrap();
    let b = reg
        .prepare(request("frontend", TaskId::new(), "b"))
        .await
        .unwrap();

    let (_verdict, _) = a.broker.evaluate(
        "req-a",
        "Bash",
        &json!({ "command": "make release" }),
        None,
        None,
        vec![],
    );

    assert_eq!(a.broker.pending_count(), 1);
    assert_eq!(
        b.broker.pending_count(),
        0,
        "agent B must not see agent A's pending prompt"
    );
    assert_eq!(
        reg.pending_permissions(),
        1,
        "the app-wide badge still needs a total across agents"
    );
}

#[tokio::test]
async fn a_permission_can_be_resolved_without_the_caller_knowing_which_agent_owns_it() {
    // Otherwise the frontend would have to track request-to-agent mapping, putting
    // safety-critical bookkeeping in the UI.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("resolve");
    let reg = WorkspaceRegistry::new(repo);

    let a = reg
        .prepare(request("backend", TaskId::new(), "a"))
        .await
        .unwrap();
    let _b = reg
        .prepare(request("frontend", TaskId::new(), "b"))
        .await
        .unwrap();

    let (verdict, _) = a.broker.evaluate(
        "req-x",
        "Bash",
        &json!({ "command": "make" }),
        None,
        None,
        vec![],
    );
    // Hold the waiter so the answer has somewhere to land.
    let _wait = match verdict {
        deck_core::permission::Verdict::Escalated { wait, .. } => Some(wait),
        _ => None,
    };

    reg.resolve_permission(
        "req-x",
        Resolution::Allowed {
            updated_input: json!({}),
        },
    )
    .expect("registry should locate the owning broker");

    assert_eq!(reg.pending_permissions(), 0);
}

#[tokio::test]
async fn resolving_an_unknown_request_is_reported() {
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("unknown");
    let reg = WorkspaceRegistry::new(repo);
    let _a = reg
        .prepare(request("backend", TaskId::new(), "a"))
        .await
        .unwrap();

    assert!(reg
        .resolve_permission(
            "nope",
            Resolution::Denied {
                message: "x".into()
            }
        )
        .is_err());
}

#[tokio::test]
async fn preparing_the_same_task_twice_reuses_the_worktree_and_its_work() {
    // A retry must not discard uncommitted work by starting a fresh tree.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("reuse");
    let reg = WorkspaceRegistry::new(repo);
    let task = TaskId::new();

    let first = reg
        .prepare(request("backend", task, "same task"))
        .await
        .unwrap();
    std::fs::write(first.worktree_path().join("progress.txt"), "half done\n").unwrap();

    let second = reg
        .prepare(request("backend", task, "same task"))
        .await
        .unwrap();

    assert_eq!(first.worktree_path(), second.worktree_path());
    assert!(
        second.worktree_path().join("progress.txt").exists(),
        "a retry must not throw away work in progress"
    );
}

#[tokio::test]
async fn an_extra_layer_cannot_widen_the_boundary() {
    // Deny is monotonic, so a caller passing a permissive layer must not be able to unlock a
    // tool the defaults deny.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("nowiden");
    let reg = WorkspaceRegistry::new(repo);

    let mut req = request("backend", TaskId::new(), "widen");
    req.extra_layers = vec![PolicyLayer {
        name: "reckless".into(),
        allow_tools: vec!["WebFetch".into()],
        allow_bash_prefixes: vec!["sudo".into()],
        ..Default::default()
    }];

    let ws = reg.prepare(req).await.unwrap();

    let (decision, _) = ws.broker.policy().decide("WebFetch", &json!({}));
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "a permissive extra layer re-enabled a denied tool: {decision:?}"
    );

    let (decision, _) = ws
        .broker
        .policy()
        .decide("Bash", &json!({ "command": "sudo rm -rf /" }));
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "a permissive extra layer allowed a denied command: {decision:?}"
    );
}

#[tokio::test]
async fn spawn_options_always_match_the_workspace() {
    // The point of building options from the workspace: a caller cannot spawn an agent with
    // someone else's containment root, or forget to attach the broker.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("spawnopts");
    let reg = WorkspaceRegistry::new(repo);
    let ws = reg
        .prepare(request("backend", TaskId::new(), "opts"))
        .await
        .unwrap();

    let opts = ws.spawn_options(deck_core::domain::ids::SessionId::new());

    assert_eq!(
        opts.config.cwd, ws.worktree.path,
        "cwd must be the agent's own worktree"
    );
    assert!(opts.broker.is_some(), "the broker must be attached");

    let argv: Vec<String> = opts
        .config
        .to_argv()
        .into_iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    assert!(
        !argv.iter().any(|a| a == "--add-dir"),
        "no --add-dir: the worktree must be the only writable root"
    );
    assert!(argv.contains(&"--permission-prompt-tool".to_string()));
    assert!(argv.contains(&"acceptEdits".to_string()));
}

#[tokio::test]
async fn forgetting_a_workspace_does_not_delete_the_agents_work() {
    // Dropping our bookkeeping must never be what destroys a worktree.
    let _s = SERIAL.lock().await;
    let repo = scratch_repo("forget");
    let reg = WorkspaceRegistry::new(repo);
    let task = TaskId::new();

    let ws = reg
        .prepare(request("backend", task, "forget"))
        .await
        .unwrap();
    let path = ws.worktree_path().to_path_buf();
    std::fs::write(path.join("work.txt"), "precious\n").unwrap();

    reg.forget(task);

    assert!(reg.get(task).is_none(), "the record should be gone");
    assert!(
        path.join("work.txt").exists(),
        "the worktree and its work must survive being forgotten"
    );
}
