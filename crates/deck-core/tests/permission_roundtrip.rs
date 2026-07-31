//! Full `can_use_tool` round-trip through the real actor.
//!
//! A stand-in CLI emits a genuine `control_request`, the broker decides, and the response is
//! read back off the child's stdin. This is the path that decides whether a human can actually
//! approve an out-of-worktree action, so it is tested against real process I/O rather than by
//! calling the broker directly.

#![cfg(unix)]

use deck_core::bus::EventBus;
use deck_core::domain::event::AgentEvent;
use deck_core::domain::ids::SessionId;
use deck_core::permission::broker::{PermissionBroker, Resolution};
use deck_core::permission::{worker_defaults, EffectivePolicy};
use deck_core::runtime::claude_code::actor::{spawn_session, SpawnOptions};
use deck_core::runtime::claude_code::argv::SessionConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// These spawn real processes; running them in parallel makes timing assertions flaky for
/// reasons unrelated to the code.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-perm-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn fake_cli(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

/// A CLI that handshakes, asks permission for one tool call, records whatever answer it gets,
/// then finishes the turn.
fn asking_cli(dir: &Path, target_path: &str, reply_file: &Path) -> PathBuf {
    let request = serde_json::json!({
        "type": "control_request",
        "request_id": "req-perm-1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Write",
            "display_name": "Write",
            "input": { "file_path": target_path, "content": "x" },
            "description": target_path,
            "tool_use_id": "toolu_test",
            "decision_reason": "Path is outside allowed working directories",
            "decision_reason_type": "workingDir",
            "blocked_path": target_path,
            "permission_suggestions": [
                { "type": "addDirectories", "directories": ["/tmp/outside"], "destination": "session" }
            ]
        }
    })
    .to_string();

    fake_cli(
        dir,
        &format!(
            "echo '{{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"/w\",\"tools\":[]}}'\n\
             echo '{request}'\n\
             IFS= read -r reply\n\
             printf '%s\\n' \"$reply\" > {}\n\
             echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false}}'",
            reply_file.display()
        ),
    )
}

type Envelope = deck_core::domain::event::EventEnvelope;
type ExitReason = deck_core::domain::event::ExitReason;

/// Handles a test must hold for the duration of a run.
///
/// The durable receiver is returned rather than dropped: dropping it closes the bounded
/// channel, and `publish` would then log every event as undeliverable instead of exercising
/// the real path.
struct Running {
    _bus: Arc<EventBus>,
    _durable: tokio::sync::mpsc::Receiver<Envelope>,
    observer: tokio::sync::broadcast::Receiver<Envelope>,
    join: tokio::task::JoinHandle<ExitReason>,
}

async fn run(dir: &Path, cli: PathBuf, broker: Option<Arc<PermissionBroker>>) -> Running {
    let (bus, durable) = EventBus::new();
    let observer = bus.subscribe();

    let mut opts = SpawnOptions::new(SessionConfig::new(SessionId::new(), dir.to_path_buf()));
    opts.program = cli.to_string_lossy().into_owned();
    opts.broker = broker;
    opts.startup_timeout = Duration::from_secs(10);

    let (_handle, join) = spawn_session(opts, bus.clone()).await.expect("spawn");
    Running {
        _bus: bus,
        _durable: durable,
        observer,
        join,
    }
}

fn broker_for(worktree: PathBuf, timeout: Duration) -> Arc<PermissionBroker> {
    let policy = EffectivePolicy::resolve(worktree, &[worker_defaults()]);
    Arc::new(PermissionBroker::with_config(
        policy,
        timeout,
        Box::new(|| 1_760_000_000_000),
    ))
}

#[tokio::test]
async fn an_operator_approval_reaches_the_agent_as_an_allow() {
    let _serial = SERIAL.lock().await;
    let dir = tempdir("approve");
    let reply = dir.join("reply.json");
    let cli = asking_cli(&dir, "/tmp/outside/escape.txt", &reply);
    let broker = broker_for(dir.clone(), Duration::from_secs(10));

    let mut running = run(&dir, cli, Some(broker.clone())).await;
    let observer = &mut running.observer;

    // Wait for the escalation to surface, exactly as the UI would.
    let escalated = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match observer.recv().await {
                Ok(env) => {
                    if let AgentEvent::PermissionRequest {
                        request_id,
                        reason_type,
                        blocked_path,
                        suggestions,
                        ..
                    } = env.event
                    {
                        return Some((request_id, reason_type, blocked_path, suggestions));
                    }
                }
                Err(_) => return None,
            }
        }
    })
    .await
    .expect("escalation should surface within the budget");

    let (request_id, reason_type, blocked_path, suggestions) =
        escalated.expect("a permission request event");

    // The CLI's own classification must survive to the UI untouched.
    assert_eq!(reason_type.as_deref(), Some("workingDir"));
    assert_eq!(blocked_path.as_deref(), Some("/tmp/outside/escape.txt"));
    assert_eq!(suggestions.len(), 1, "structured options must reach the UI");

    broker
        .resolve(
            &request_id,
            Resolution::Allowed {
                updated_input: serde_json::json!({
                    "file_path": "/tmp/outside/escape.txt", "content": "x"
                }),
            },
        )
        .expect("operator approval");

    let _ = tokio::time::timeout(Duration::from_secs(10), running.join).await;

    let written = std::fs::read_to_string(&reply).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(written.trim()).expect("reply is JSON");

    assert_eq!(parsed["type"], "control_response");
    assert_eq!(parsed["response"]["request_id"], request_id);
    assert_eq!(
        parsed["response"]["response"]["behavior"], "allow",
        "approval must reach the CLI as an allow: {parsed}"
    );
}

#[tokio::test]
async fn a_policy_denied_call_is_refused_without_ever_asking_the_operator() {
    let _serial = SERIAL.lock().await;
    let dir = tempdir("autodeny");
    let reply = dir.join("reply.json");
    // ~/.ssh is denied outright by policy, so no human should be involved at all.
    let cli = asking_cli(&dir, "/Users/dev/.ssh/id_rsa", &reply);
    let broker = broker_for(dir.clone(), Duration::from_secs(10));

    let running = run(&dir, cli, Some(broker.clone())).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), running.join).await;

    assert_eq!(
        broker.pending_count(),
        0,
        "a policy denial must not queue a prompt for the operator"
    );

    let written = std::fs::read_to_string(&reply).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(written.trim()).expect("reply is JSON");

    assert_eq!(parsed["response"]["response"]["behavior"], "deny");
    let message = parsed["response"]["response"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(
        message.len() > 30,
        "the agent must be told why, or it retries the same call: {message:?}"
    );
}

#[tokio::test]
async fn an_unanswered_prompt_becomes_an_explained_denial_rather_than_hanging_the_agent() {
    let _serial = SERIAL.lock().await;
    let dir = tempdir("timeout");
    let reply = dir.join("reply.json");
    let cli = asking_cli(&dir, "/tmp/outside/escape.txt", &reply);
    // Short window so the test does not wait out the production timeout.
    let broker = broker_for(dir.clone(), Duration::from_millis(400));

    let running = run(&dir, cli, Some(broker.clone())).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), running.join).await;

    let written = std::fs::read_to_string(&reply).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(written.trim()).expect("reply is JSON");

    assert_eq!(
        parsed["response"]["response"]["behavior"], "deny",
        "an ignored prompt must fail closed, never open"
    );
    assert_eq!(
        broker.pending_count(),
        0,
        "expired requests must be cleaned up, not leaked"
    );
}

#[tokio::test]
async fn with_no_broker_configured_the_actor_leaves_the_request_alone() {
    // Without a broker the CLI's own acceptEdits handling applies. The actor must not invent an
    // answer, since silently allowing would remove the containment guarantee entirely.
    let _serial = SERIAL.lock().await;
    let dir = tempdir("nobroker");
    let reply = dir.join("reply.json");
    let cli = asking_cli(&dir, "/tmp/outside/escape.txt", &reply);

    let running = run(&dir, cli, None).await;
    let _ = tokio::time::timeout(Duration::from_secs(6), running.join).await;

    let written = std::fs::read_to_string(&reply).unwrap_or_default();
    assert!(
        written.trim().is_empty(),
        "no broker means no answer should be written, got {written:?}"
    );
}
