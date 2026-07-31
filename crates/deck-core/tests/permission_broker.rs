//! Broker behaviour, focused on the failure modes that would either wedge an agent or let one
//! through unsupervised.

use deck_core::permission::broker::{await_resolution, PermissionBroker, Resolution, Verdict};
use deck_core::permission::{worker_defaults, EffectivePolicy};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn worktree() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-broker-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn broker(timeout: Duration) -> Arc<PermissionBroker> {
    let policy = EffectivePolicy::resolve(worktree(), &[worker_defaults()]);
    Arc::new(PermissionBroker::with_config(
        policy,
        timeout,
        Box::new(|| 1_760_000_000_000),
    ))
}

#[tokio::test]
async fn policy_allowable_calls_never_reach_a_human() {
    // If routine in-tree work escalated, the operator would drown in prompts and autonomy
    // would be worthless.
    let b = broker(Duration::from_secs(60));
    let target = b.policy().worktree().join("notes.md");

    let (verdict, _) = b.evaluate(
        "r1",
        "Write",
        &json!({ "file_path": target, "content": "hi" }),
        None,
        None,
        vec![],
    );

    match verdict {
        Verdict::Immediate(Resolution::Allowed { .. }) => {}
        other => panic!(
            "in-tree write should be settled without asking, got {:?}",
            matches!(other, Verdict::Escalated { .. })
        ),
    }
    assert_eq!(b.pending_count(), 0, "nothing should be parked");
}

#[tokio::test]
async fn policy_denials_are_immediate_and_carry_an_explanation() {
    let b = broker(Duration::from_secs(60));
    let (verdict, _) = b.evaluate("r1", "WebFetch", &json!({}), None, None, vec![]);

    match verdict {
        Verdict::Immediate(Resolution::Denied { message }) => {
            assert!(message.len() > 30, "denial must be actionable: {message}");
        }
        _ => panic!("expected an immediate denial"),
    }
}

#[tokio::test]
async fn an_escalation_carries_the_clis_own_classification_through_to_the_ui() {
    // The CLI tells us why it asked and which path was blocked. Passing that through means the
    // UI can explain the prompt precisely instead of guessing.
    let b = broker(Duration::from_secs(60));
    let (verdict, _) = b.evaluate(
        "r1",
        "Write",
        &json!({ "file_path": "/tmp/outside/x.txt", "content": "x" }),
        Some("workingDir".into()),
        Some("/tmp/outside/x.txt".into()),
        vec![json!({ "type": "addDirectories", "directories": ["/tmp/outside"] })],
    );

    match verdict {
        Verdict::Escalated { request, .. } => {
            assert_eq!(request.cli_reason_type.as_deref(), Some("workingDir"));
            assert_eq!(request.blocked_path.as_deref(), Some("/tmp/outside/x.txt"));
            assert_eq!(
                request.suggestions.len(),
                1,
                "structured options must survive to the UI so it never parses prose"
            );
            assert!(
                request.expires_at_ms > 1_760_000_000_000,
                "a deadline is required so the UI can show a countdown"
            );
        }
        _ => panic!("out-of-tree write should escalate"),
    }
    assert_eq!(b.pending_count(), 1);
}

#[tokio::test]
async fn a_human_approval_unblocks_the_waiting_agent() {
    let b = broker(Duration::from_secs(60));
    let (verdict, _) = b.evaluate(
        "r1",
        "Bash",
        &json!({ "command": "make release" }),
        None,
        None,
        vec![],
    );

    let Verdict::Escalated { wait, request } = verdict else {
        panic!("expected escalation");
    };

    let approver = {
        let b = b.clone();
        tokio::spawn(async move {
            b.resolve(
                "r1",
                Resolution::Allowed {
                    updated_input: json!({ "command": "make release" }),
                },
            )
        })
    };

    let resolution = await_resolution(b.clone(), request.request_id, wait).await;
    approver.await.unwrap().expect("resolve should succeed");

    assert!(matches!(resolution, Resolution::Allowed { .. }));
    assert_eq!(b.pending_count(), 0, "resolved requests must not leak");
}

#[tokio::test]
async fn an_unanswered_request_is_denied_with_an_explanation_not_left_hanging() {
    // A forgotten prompt must not wedge an agent forever, and the denial must not look like a
    // tool malfunction or the model will retry the identical call.
    let b = broker(Duration::from_millis(150));
    let (verdict, _) = b.evaluate(
        "r1",
        "Bash",
        &json!({ "command": "make release" }),
        None,
        None,
        vec![],
    );
    let Verdict::Escalated { wait, request } = verdict else {
        panic!("expected escalation");
    };

    let resolution = await_resolution(b.clone(), request.request_id, wait).await;

    match resolution {
        Resolution::Denied { message } => {
            assert!(
                message.contains("declined") && message.len() > 40,
                "timeout denial must explain itself: {message}"
            );
        }
        Resolution::Allowed { .. } => {
            panic!("a timeout must never fail open")
        }
    }
    assert_eq!(b.pending_count(), 0, "expired requests must be cleaned up");
}

#[tokio::test]
async fn answering_twice_is_reported_rather_than_panicking() {
    // Double-clicking Approve is expected user behaviour, not a bug to crash on.
    let b = broker(Duration::from_secs(60));
    let (verdict, _) = b.evaluate(
        "r1",
        "Bash",
        &json!({ "command": "make" }),
        None,
        None,
        vec![],
    );
    let Verdict::Escalated { wait, .. } = verdict else {
        panic!("expected escalation");
    };

    b.resolve(
        "r1",
        Resolution::Denied {
            message: "no".into(),
        },
    )
    .expect("first answer");
    let second = b.resolve(
        "r1",
        Resolution::Denied {
            message: "no".into(),
        },
    );
    assert!(
        second.is_err(),
        "second answer should be reported, not silently accepted"
    );

    drop(wait);
}

#[tokio::test]
async fn resolving_an_unknown_id_is_an_error_not_a_silent_noop() {
    // Silently accepting an unknown id would hide a UI/backend mismatch where prompts appear
    // to be answered but agents stay blocked.
    let b = broker(Duration::from_secs(60));
    assert!(b
        .resolve(
            "never-existed",
            Resolution::Denied {
                message: "x".into()
            }
        )
        .is_err());
}

#[tokio::test]
async fn concurrent_escalations_stay_independent() {
    // Several agents can be blocked at once; answering one must not disturb the others.
    let b = broker(Duration::from_secs(60));
    let mut waits = Vec::new();

    for i in 0..5 {
        let id = format!("r{i}");
        let (verdict, _) = b.evaluate(
            &id,
            "Bash",
            &json!({ "command": format!("make target{i}") }),
            None,
            None,
            vec![],
        );
        let Verdict::Escalated { wait, request } = verdict else {
            panic!("expected escalation");
        };
        waits.push((request.request_id, wait));
    }
    assert_eq!(b.pending_count(), 5);

    // Answer the middle one only.
    b.resolve(
        "r2",
        Resolution::Allowed {
            updated_input: json!({}),
        },
    )
    .unwrap();

    assert_eq!(
        b.pending_count(),
        4,
        "only the answered request should clear"
    );
    assert!(b.is_pending("r0") && b.is_pending("r4"));

    let (_, wait) = waits.into_iter().find(|(id, _)| id == "r2").unwrap();
    assert!(matches!(wait.await, Ok(Resolution::Allowed { .. })));
}
