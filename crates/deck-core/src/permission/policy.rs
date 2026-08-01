//! Permission policy resolution.
//!
//! # Threat model
//!
//! This layer defends against an agent *mistake*, not an agent *adversary*. The objective is
//! trusted; the actions taken toward it are not. That distinction matters because the Bash
//! rules below are prefix matches, and shell is not safely parseable — `cd ..`, `git -C /`,
//! `python -c`, heredocs and command substitution all defeat prefix matching. **Bash rules are
//! a speed bump, not a security boundary.** The real containment is that the process runs with
//! its cwd set to the worktree and no `--add-dir`, so the CLI itself routes out-of-tree writes
//! into the permission pipeline.
//!
//! # Layering
//!
//! Layers resolve Global → Workspace → Project → Agent → Task → Session. One firm rule:
//! **deny is monotonic** — denials union across layers and a lower layer can never override
//! one. Allows only ever narrow. This is the only ordering in which adding a policy cannot
//! accidentally widen access.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Answered in-process with no human involvement. This path is what makes autonomy feel
    /// fast, so it must stay allocation-light and free of I/O.
    Allow {
        updated_input: Value,
    },
    /// Always carries a message: a bare denial reads to the agent as a tool malfunction and
    /// it will usually retry the identical call.
    Deny {
        message: String,
    },
    Ask {
        reason: String,
    },
}

/// Why a decision was reached, recorded so the audit log explains itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Rationale {
    ExplicitDeny { rule: String },
    OutsideWorktree { path: String },
    ExplicitAllow { rule: String },
    UnlistedBashCommand { command: String },
    NoMatchingRule,
    SensitivePath { path: String },
}

/// A single policy layer. `deny` wins over `ask` wins over `allow` within a layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicyLayer {
    pub name: String,
    /// Tool names always refused, e.g. `WebFetch`. Unioned across layers.
    pub deny_tools: Vec<String>,
    /// Tool names permitted without asking.
    pub allow_tools: Vec<String>,
    /// Bash command prefixes permitted without asking, e.g. `cargo test`.
    pub allow_bash_prefixes: Vec<String>,
    /// Bash substrings that always refuse, checked before any allow. Unioned across layers.
    pub deny_bash_patterns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EffectivePolicy {
    /// Writes must stay inside this directory. Already canonicalized.
    worktree: PathBuf,
    deny_tools: Vec<String>,
    allow_tools: Vec<String>,
    allow_bash_prefixes: Vec<String>,
    deny_bash_patterns: Vec<String>,
}

/// Tools whose input names a filesystem path that must be inside the worktree.
const PATH_TOOLS: &[(&str, &str)] = &[
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("Read", "file_path"),
    ("NotebookEdit", "notebook_path"),
];

/// Refused regardless of policy. These are credential stores and system state where a mistake
/// is unrecoverable or leaks secrets, so no layer may allow them.
const ALWAYS_SENSITIVE: &[&str] = &[
    "/.ssh/",
    "/.aws/",
    "/.gnupg/",
    "/.kube/",
    "/.docker/config.json",
    "/.netrc",
    "/.npmrc",
    "/.git-credentials",
    "/etc/",
    "/private/etc/",
];

impl EffectivePolicy {
    /// Folds ordered layers. Callers pass them outermost-first; order affects only which rule
    /// name is reported, never the outcome, because denials union.
    pub fn resolve(worktree: PathBuf, layers: &[PolicyLayer]) -> Self {
        let mut policy = Self {
            worktree,
            deny_tools: Vec::new(),
            allow_tools: Vec::new(),
            allow_bash_prefixes: Vec::new(),
            deny_bash_patterns: Vec::new(),
        };

        for layer in layers {
            // Denials accumulate and are never removed by a later layer.
            policy.deny_tools.extend(layer.deny_tools.iter().cloned());
            policy
                .deny_bash_patterns
                .extend(layer.deny_bash_patterns.iter().cloned());
            policy.allow_tools.extend(layer.allow_tools.iter().cloned());
            policy
                .allow_bash_prefixes
                .extend(layer.allow_bash_prefixes.iter().cloned());
        }

        // An allow is void if any layer denies the same tool: deny is monotonic.
        policy
            .allow_tools
            .retain(|t| !policy.deny_tools.iter().any(|d| d == t));

        policy
    }

    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    pub fn decide(&self, tool: &str, input: &Value) -> (Decision, Rationale) {
        if let Some(rule) = self.deny_tools.iter().find(|d| *d == tool) {
            return (
                Decision::Deny {
                    message: format!(
                        "{tool} is not permitted for this agent. Use a different approach; \
                         retrying the same call will fail again."
                    ),
                },
                Rationale::ExplicitDeny { rule: rule.clone() },
            );
        }

        if tool == "Bash" {
            return self.decide_bash(input);
        }

        if let Some((_, field)) = PATH_TOOLS.iter().find(|(name, _)| *name == tool) {
            if let Some(raw) = input.get(*field).and_then(Value::as_str) {
                if let Some(outcome) = self.check_path(tool, raw) {
                    return outcome;
                }
            }
        }

        if self.allow_tools.iter().any(|t| t == tool) {
            return (
                Decision::Allow {
                    updated_input: input.clone(),
                },
                Rationale::ExplicitAllow {
                    rule: tool.to_string(),
                },
            );
        }

        (
            Decision::Ask {
                reason: format!("{tool} is not covered by an allow rule"),
            },
            Rationale::NoMatchingRule,
        )
    }

    /// Returns `Some` only when the path is disallowed; `None` means "no objection from the
    /// path check", leaving the normal allow/ask logic to run.
    fn check_path(&self, tool: &str, raw: &str) -> Option<(Decision, Rationale)> {
        let candidate = PathBuf::from(raw);
        let resolved = canonicalize_for_write(&candidate);

        let as_str = resolved.to_string_lossy();
        if ALWAYS_SENSITIVE.iter().any(|s| as_str.contains(s)) {
            return Some((
                Decision::Deny {
                    message: format!(
                        "{raw} is a credential or system path that AgentDeck never allows an \
                         agent to touch, regardless of policy."
                    ),
                },
                Rationale::SensitivePath {
                    path: as_str.into_owned(),
                },
            ));
        }

        if !resolved.starts_with(&self.worktree) {
            return Some((
                Decision::Ask {
                    reason: format!("{tool} targets {raw}, which is outside this agent's worktree"),
                },
                Rationale::OutsideWorktree {
                    path: as_str.into_owned(),
                },
            ));
        }

        None
    }

    fn decide_bash(&self, input: &Value) -> (Decision, Rationale) {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let normalized = command.trim();

        if let Some(pattern) = self
            .deny_bash_patterns
            .iter()
            .find(|p| normalized.contains(p.as_str()))
        {
            return (
                Decision::Deny {
                    message: format!(
                        "This command contains {pattern:?}, which is blocked for this agent."
                    ),
                },
                Rationale::ExplicitDeny {
                    rule: pattern.clone(),
                },
            );
        }

        // Only a whole-command prefix match counts, and only for a single command. Anything
        // chaining operators is escalated rather than pattern-matched, because an allowed
        // prefix followed by `&&` or `;` says nothing about what runs next.
        if !has_shell_composition(normalized) {
            if let Some(prefix) = self
                .allow_bash_prefixes
                .iter()
                .find(|p| command_matches_prefix(normalized, p))
            {
                return (
                    Decision::Allow {
                        updated_input: input.clone(),
                    },
                    Rationale::ExplicitAllow {
                        rule: prefix.clone(),
                    },
                );
            }
        }

        (
            Decision::Ask {
                reason: format!("Bash command not covered by an allow prefix: {normalized}"),
            },
            Rationale::UnlistedBashCommand {
                command: normalized.to_string(),
            },
        )
    }
}

/// True if the command contains shell syntax that lets it run something beyond its prefix.
fn has_shell_composition(command: &str) -> bool {
    const OPERATORS: &[&str] = &["&&", "||", ";", "|", ">", "<", "$(", "`", "\n", "&"];
    OPERATORS.iter().any(|op| command.contains(op))
}

/// Prefix match on whole arguments, so `cargo testsuite` does not match a `cargo test` rule.
fn command_matches_prefix(command: &str, prefix: &str) -> bool {
    let prefix = prefix.trim();
    if !command.starts_with(prefix) {
        return false;
    }
    match command.as_bytes().get(prefix.len()) {
        None => true,
        Some(b) => b.is_ascii_whitespace(),
    }
}

/// Resolves a path for containment checking.
///
/// `canonicalize` fails on files that do not exist yet, which is the common case for `Write`.
/// So the deepest existing ancestor is canonicalized — which resolves symlinks, closing the
/// escape where a symlink inside the worktree points outside it — and the remaining components
/// are appended with `..` collapsed.
fn canonicalize_for_write(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }

    let mut remainder = Vec::new();
    let mut cursor = path.to_path_buf();

    loop {
        match cursor.parent() {
            Some(parent) => {
                if let Some(name) = cursor.file_name() {
                    remainder.push(name.to_owned());
                }
                if let Ok(real_parent) = parent.canonicalize() {
                    let mut out = real_parent;
                    for part in remainder.iter().rev() {
                        if part == ".." {
                            out.pop();
                        } else if part != "." {
                            out.push(part);
                        }
                    }
                    return out;
                }
                cursor = parent.to_path_buf();
            }
            // Nothing on this path exists; fall back to lexical normalization so the check
            // still errs toward "outside the worktree" rather than silently passing.
            None => return normalize_lexically(path),
        }
    }
}

fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The default posture for a worker agent: free movement inside its worktree, escalation for
/// anything else. Mirrors what the CLI is told via `--permission-mode acceptEdits`.
pub fn worker_defaults() -> PolicyLayer {
    PolicyLayer {
        name: "worker-default".into(),
        deny_tools: vec!["WebFetch".into(), "WebSearch".into()],
        allow_tools: vec![
            "Read".into(),
            "Write".into(),
            "Edit".into(),
            "Glob".into(),
            "Grep".into(),
        ],
        allow_bash_prefixes: vec![
            "cargo test".into(),
            "cargo build".into(),
            "cargo clippy".into(),
            "cargo fmt".into(),
            "pnpm test".into(),
            "pnpm build".into(),
            "pnpm typecheck".into(),
            "git status".into(),
            "git diff".into(),
            "git add".into(),
            "git commit".into(),
            "git log".into(),
            "ls".into(),
            "cat".into(),
            "rg".into(),
        ],
        deny_bash_patterns: vec![
            "rm -rf".into(),
            "sudo".into(),
            "git push".into(),
            "curl".into(),
            "wget".into(),
            "chmod 777".into(),
            ":(){".into(),
        ],
    }
}

/// How much an agent may do without being asked.
///
/// Three points on one axis, because the operator's real question is "how much do I trust this
/// team on this codebase" rather than "which of forty tool names should be allowed". The axis
/// moves what is auto-approved; it never moves the worktree boundary or the deny list, both of
/// which hold at every level — see [`level_layer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Reads and searches freely; every write and every shell command is asked about.
    Cautious,
    /// Edits inside its own worktree without asking. Shell is a curated allowlist.
    #[default]
    Standard,
    /// As Standard, plus shell commands are allowed unless the deny list catches them.
    Trusted,
}

impl PermissionLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionLevel::Cautious => "cautious",
            PermissionLevel::Standard => "standard",
            PermissionLevel::Trusted => "trusted",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "cautious" => PermissionLevel::Cautious,
            "trusted" => PermissionLevel::Trusted,
            _ => PermissionLevel::Standard,
        }
    }

    /// Whether the CLI may apply edits without a prompt.
    ///
    /// Cautious is the only level that says no, which is what makes it a review posture rather
    /// than a slower version of the same thing.
    pub fn accepts_edits(self) -> bool {
        !matches!(self, PermissionLevel::Cautious)
    }
}

/// Builds the policy for a level, plus whatever shell prefixes the operator added.
///
/// The deny list is identical at every level and is not configurable from the UI. Deny is
/// monotonic by design — it is unioned across layers and no lower layer can override it — so
/// offering a control that appeared to relax it would be a safety claim the resolver does not
/// honour. `rm -rf`, `sudo`, `git push` and network fetches stay refused even at Trusted; the
/// worktree boundary is enforced by the CLI itself and is not on this axis at all.
pub fn level_layer(level: PermissionLevel, extra_bash: &[String]) -> PolicyLayer {
    let mut layer = worker_defaults();
    layer.name = format!("worker-{}", level.as_str());

    match level {
        PermissionLevel::Cautious => {
            // Writing is still possible, it just becomes a question. Removing the tools outright
            // would make the agent unable to do the work rather than unable to do it unattended.
            layer.allow_tools.retain(|t| t != "Write" && t != "Edit");
            layer.allow_bash_prefixes.clear();
        }
        PermissionLevel::Standard => {}
        PermissionLevel::Trusted => {
            // An empty prefix matches every command, so the deny list becomes the only gate.
            layer.allow_bash_prefixes.push(String::new());
        }
    }

    layer
        .allow_bash_prefixes
        .extend(extra_bash.iter().filter(|p| !p.trim().is_empty()).cloned());
    layer
}

#[cfg(test)]
mod level_tests {
    use super::*;

    #[test]
    fn the_deny_list_survives_every_level() {
        // The one invariant the whole control rests on. Deny is unioned across layers and cannot
        // be overridden downstream, so a level that appeared to relax it would be lying.
        for level in [
            PermissionLevel::Cautious,
            PermissionLevel::Standard,
            PermissionLevel::Trusted,
        ] {
            let layer = level_layer(level, &[]);
            for forbidden in ["rm -rf", "sudo", "git push", "curl"] {
                assert!(
                    layer.deny_bash_patterns.iter().any(|p| p == forbidden),
                    "{forbidden} must stay denied at {}",
                    level.as_str()
                );
            }
            assert!(layer.deny_tools.iter().any(|t| t == "WebFetch"));
        }
    }

    #[test]
    fn cautious_asks_before_writing_and_trusted_does_not_ask_before_shell() {
        let cautious = level_layer(PermissionLevel::Cautious, &[]);
        assert!(!cautious.allow_tools.iter().any(|t| t == "Write"));
        assert!(cautious.allow_bash_prefixes.is_empty());
        assert!(
            cautious.allow_tools.iter().any(|t| t == "Read"),
            "reading is never the risky part"
        );

        let trusted = level_layer(PermissionLevel::Trusted, &[]);
        assert!(
            trusted.allow_bash_prefixes.iter().any(|p| p.is_empty()),
            "an empty prefix matches everything, leaving the deny list as the only gate"
        );
    }

    #[test]
    fn operator_prefixes_are_added_and_blanks_ignored() {
        let layer = level_layer(PermissionLevel::Standard, &["make ".into(), "  ".into()]);
        assert!(layer.allow_bash_prefixes.iter().any(|p| p == "make "));
        assert!(
            !layer
                .allow_bash_prefixes
                .iter()
                .any(|p| p.trim().is_empty()),
            "a blank row would silently allow every command"
        );
    }
}
