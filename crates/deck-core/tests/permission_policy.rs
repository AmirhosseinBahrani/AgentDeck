//! Adversarial tests for the permission policy.
//!
//! This is the layer that decides whether an agent's mistake stays contained, so the tests are
//! written as attempts to escape rather than as confirmations that the happy path works.

use deck_core::permission::{worker_defaults, Decision, EffectivePolicy, PolicyLayer, Rationale};
use serde_json::json;
use std::path::PathBuf;

fn worktree() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agentdeck-policy-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
    // Canonicalized because /var vs /private/var on macOS would otherwise make every
    // containment check fail for the wrong reason.
    dir.canonicalize().unwrap()
}

fn policy(root: PathBuf) -> EffectivePolicy {
    EffectivePolicy::resolve(root, &[worker_defaults()])
}

fn write_to(path: &str) -> serde_json::Value {
    json!({ "file_path": path, "content": "x" })
}

fn bash(command: &str) -> serde_json::Value {
    json!({ "command": command })
}

// ---------------------------------------------------------------------------
// Containment
// ---------------------------------------------------------------------------

#[test]
fn writes_inside_the_worktree_are_allowed_without_asking() {
    let root = worktree();
    let p = policy(root.clone());
    let target = root.join("src/new_file.rs");
    let (decision, _) = p.decide("Write", &write_to(target.to_str().unwrap()));
    assert!(
        matches!(decision, Decision::Allow { .. }),
        "in-tree writes must not prompt, or autonomy is unusable: {decision:?}"
    );
}

#[test]
fn writes_outside_the_worktree_escalate() {
    let root = worktree();
    let p = policy(root);
    let (decision, rationale) = p.decide("Write", &write_to("/tmp/elsewhere/escape.txt"));
    assert!(matches!(decision, Decision::Ask { .. }), "got {decision:?}");
    assert!(matches!(rationale, Rationale::OutsideWorktree { .. }));
}

#[test]
fn parent_traversal_out_of_the_worktree_is_caught() {
    // The obvious escape: a path that starts inside and climbs out.
    let root = worktree();
    let p = policy(root.clone());
    let sneaky = format!("{}/src/../../outside.txt", root.display());
    let (decision, rationale) = p.decide("Write", &write_to(&sneaky));
    assert!(
        matches!(decision, Decision::Ask { .. }),
        "`..` traversal escaped containment: {decision:?}"
    );
    assert!(matches!(rationale, Rationale::OutsideWorktree { .. }));
}

#[test]
fn traversal_that_stays_inside_is_still_allowed() {
    // Containment must not be so blunt that legitimate relative paths break.
    let root = worktree();
    let p = policy(root.clone());
    let winding = format!("{}/src/../src/ok.rs", root.display());
    let (decision, _) = p.decide("Write", &write_to(&winding));
    assert!(
        matches!(decision, Decision::Allow { .. }),
        "got {decision:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_worktree_pointing_out_does_not_grant_access() {
    // The subtle escape, and the reason containment canonicalizes rather than string-matches:
    // a symlink whose path prefix looks in-tree but resolves elsewhere.
    let root = worktree();
    let outside = std::env::temp_dir().join(format!("agentdeck-outside-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&outside).unwrap();
    let link = root.join("escape-hatch");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let p = policy(root);
    let through_link = link.join("secret.txt");
    let (decision, rationale) = p.decide("Write", &write_to(through_link.to_str().unwrap()));

    assert!(
        matches!(decision, Decision::Ask { .. }),
        "symlink escape was permitted: {decision:?} / {rationale:?}"
    );
}

#[test]
fn credential_paths_are_denied_outright_and_cannot_be_allowed_by_policy() {
    let root = worktree();
    // Even a maximally permissive layer must not open these.
    let permissive = PolicyLayer {
        name: "reckless".into(),
        allow_tools: vec!["Read".into(), "Write".into()],
        ..Default::default()
    };
    let p = EffectivePolicy::resolve(root, &[worker_defaults(), permissive]);

    for path in [
        "/Users/dev/.ssh/id_rsa",
        "/Users/dev/.aws/credentials",
        "/Users/dev/.git-credentials",
        "/etc/passwd",
    ] {
        let (decision, rationale) = p.decide("Read", &json!({ "file_path": path }));
        assert!(
            matches!(decision, Decision::Deny { .. }),
            "{path} should be denied outright, got {decision:?}"
        );
        assert!(matches!(rationale, Rationale::SensitivePath { .. }));
    }
}

// ---------------------------------------------------------------------------
// Deny monotonicity
// ---------------------------------------------------------------------------

#[test]
fn a_lower_layer_cannot_re_allow_what_a_higher_layer_denied() {
    // The core invariant: adding a policy must never be able to widen access.
    let root = worktree();
    let strict = PolicyLayer {
        name: "workspace".into(),
        deny_tools: vec!["Write".into()],
        ..Default::default()
    };
    let permissive = PolicyLayer {
        name: "agent".into(),
        allow_tools: vec!["Write".into()],
        ..Default::default()
    };

    let p = EffectivePolicy::resolve(root.clone(), &[strict.clone(), permissive.clone()]);
    let target = root.join("src/x.rs");
    let (decision, _) = p.decide("Write", &write_to(target.to_str().unwrap()));
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "a later allow overrode an earlier deny: {decision:?}"
    );

    // And the outcome must not depend on layer order.
    let reversed = EffectivePolicy::resolve(root.clone(), &[permissive, strict]);
    let (decision, _) = reversed.decide("Write", &write_to(target.to_str().unwrap()));
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "deny/allow resolution is order-dependent: {decision:?}"
    );
}

#[test]
fn denials_from_separate_layers_accumulate() {
    let root = worktree();
    let a = PolicyLayer {
        name: "a".into(),
        deny_bash_patterns: vec!["sudo".into()],
        ..Default::default()
    };
    let b = PolicyLayer {
        name: "b".into(),
        deny_bash_patterns: vec!["docker".into()],
        ..Default::default()
    };
    let p = EffectivePolicy::resolve(root, &[a, b]);

    for cmd in ["sudo ls", "docker ps"] {
        let (decision, _) = p.decide("Bash", &bash(cmd));
        assert!(
            matches!(decision, Decision::Deny { .. }),
            "{cmd} should be denied by the unioned patterns"
        );
    }
}

// ---------------------------------------------------------------------------
// Bash — explicitly a speed bump, but it must not be trivially bypassable
// ---------------------------------------------------------------------------

#[test]
fn allowlisted_bash_prefixes_run_without_asking() {
    let p = policy(worktree());
    for cmd in ["cargo test", "cargo test --workspace", "git status"] {
        let (decision, _) = p.decide("Bash", &bash(cmd));
        assert!(
            matches!(decision, Decision::Allow { .. }),
            "{cmd} should be allowed, got {decision:?}"
        );
    }
}

#[test]
fn a_prefix_rule_does_not_match_a_longer_command_name() {
    // `cargo test` must not authorize `cargo testsuite`, which is a different binary.
    let p = policy(worktree());
    let (decision, _) = p.decide("Bash", &bash("cargo testsuite --release"));
    assert!(
        matches!(decision, Decision::Ask { .. }),
        "prefix matching leaked across a word boundary: {decision:?}"
    );
}

#[test]
fn chaining_onto_an_allowed_prefix_does_not_inherit_its_permission() {
    // The bypass that makes naive prefix matching useless: an allowed command followed by
    // something else entirely.
    let p = policy(worktree());
    for cmd in [
        "cargo test && rm -rf /",
        "git status; cat ~/.ssh/id_rsa",
        "ls | sh",
        "cargo test $(whoami)",
        "ls > /etc/hosts",
        "cargo test\nsudo reboot",
        "ls & sudo rm -rf /",
        "cat `whoami`",
    ] {
        let (decision, _) = p.decide("Bash", &bash(cmd));
        assert!(
            !matches!(decision, Decision::Allow { .. }),
            "composition bypassed the allowlist: {cmd:?} -> {decision:?}"
        );
    }
}

#[test]
fn denied_patterns_beat_allowed_prefixes() {
    let p = policy(worktree());
    // `git add` is allowed, `git push` is denied; the deny must win on the same command line.
    let (decision, _) = p.decide("Bash", &bash("git push origin main"));
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "got {decision:?}"
    );
}

#[test]
fn unlisted_bash_commands_escalate_rather_than_being_denied() {
    // Escalation keeps the agent useful; a hard deny on anything unlisted would make the
    // human's allowlist the bottleneck for all work.
    let p = policy(worktree());
    let (decision, rationale) = p.decide("Bash", &bash("make release"));
    assert!(matches!(decision, Decision::Ask { .. }), "got {decision:?}");
    assert!(matches!(rationale, Rationale::UnlistedBashCommand { .. }));
}

// ---------------------------------------------------------------------------
// Message quality
// ---------------------------------------------------------------------------

#[test]
fn denials_always_explain_themselves() {
    // A bare denial reads to the model as a tool malfunction, so it retries the same call and
    // burns turns. Every deny must say why and imply what to do instead.
    let p = policy(worktree());
    let cases = vec![
        p.decide("WebFetch", &json!({ "url": "https://example.com" }))
            .0,
        p.decide("Bash", &bash("sudo rm -rf /")).0,
        p.decide("Read", &json!({ "file_path": "/Users/dev/.ssh/id_rsa" }))
            .0,
    ];

    for decision in cases {
        match decision {
            Decision::Deny { message } => {
                assert!(
                    message.len() > 30,
                    "denial message too terse to be actionable: {message:?}"
                );
            }
            other => panic!("expected a denial, got {other:?}"),
        }
    }
}

#[test]
fn network_tools_are_denied_by_default() {
    let p = policy(worktree());
    for tool in ["WebFetch", "WebSearch"] {
        let (decision, _) = p.decide(tool, &json!({}));
        assert!(
            matches!(decision, Decision::Deny { .. }),
            "{tool} should be off by default for a worker agent"
        );
    }
}

#[test]
fn an_unknown_tool_escalates_instead_of_being_allowed() {
    // Fail closed: a tool added by a future CLI version must not be silently permitted.
    let p = policy(worktree());
    let (decision, rationale) = p.decide("SomeFutureTool", &json!({}));
    assert!(
        matches!(decision, Decision::Ask { .. }),
        "unknown tools must not default to allow: {decision:?}"
    );
    assert!(matches!(rationale, Rationale::NoMatchingRule));
}
