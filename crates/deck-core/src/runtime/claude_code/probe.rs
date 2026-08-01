//! Is the runtime actually usable?
//!
//! AgentDeck has no accounts of its own and needs no API key — spawned `claude` processes
//! inherit OAuth credentials from the OS keychain. So there is nothing to log *in* to here, and
//! a sign-in screen would be inventing a concept the product does not have.
//!
//! What it does have is a dependency it cannot satisfy itself: the `claude` CLI has to be
//! installed and someone has to have logged into it. Without this check the operator discovers
//! that only after writing an objective, starting a run, and watching an agent fail to spawn —
//! at which point the error is attributed to their objective rather than to their machine.
//!
//! The probe runs the same two commands a person would run by hand, in the same stripped
//! environment agents get, so it tests the thing that will actually happen rather than an
//! approximation of it.

use super::actor::ENV_ALLOWLIST;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;

/// How long either probe command may take before we stop waiting.
///
/// Short: these are local and immediate. A hang means something is wrong with the install, and
/// reporting that is more useful than a spinner that never resolves.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Readiness {
    /// Installed, logged in, and ready to run agents.
    Ready {
        version: String,
        auth: AuthInfo,
        /// Where it was found. Worth showing: on a GUI launch this is the difference between
        /// the CLI the operator expects and one on a PATH they cannot see.
        path: String,
    },
    /// The CLI is not on PATH. Nothing else can be determined.
    NotInstalled { program: String },
    /// Installed but nobody has logged in. Fixed outside AgentDeck, by the operator.
    NotAuthenticated { version: String },
    /// The probe itself failed. Distinct from a definite "no", because reporting a broken probe
    /// as "not installed" would send the operator to reinstall something that is already there.
    Unknown { detail: String },
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, Readiness::Ready { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthInfo {
    pub method: Option<String>,
    pub email: Option<String>,
    /// Surfaced because on subscription billing this, not a dollar budget, is what actually
    /// limits how many agents can run at once.
    pub subscription: Option<String>,
    pub organization: Option<String>,
}

/// What `claude auth status` prints. Unknown fields are ignored so a new one cannot break the
/// check that decides whether the app is usable at all.
///
/// The CLI emits camelCase. Getting this wrong would deserialize `logged_in` as its `false`
/// default and tell a logged-in operator to log in again — the same shape of bug that once made
/// rate-limit resets silently invisible, which is why it is pinned by a test against captured
/// output rather than trusted.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthStatus {
    #[serde(default)]
    logged_in: bool,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(default)]
    org_name: Option<String>,
}

/// Checks the CLI named `program`, the same name agents will be spawned with.
pub async fn probe(program: &str) -> Readiness {
    // Repairs PATH first if a GUI launch stripped it. Without this the probe would report a
    // perfectly good install as missing, and agents would fail to spawn for the same reason.
    let resolved = crate::runtime::shell_path::ensure_tool_on_path(program).await;
    let Some(resolved) = resolved else {
        return Readiness::NotInstalled {
            program: program.to_string(),
        };
    };
    let located = resolved.display().to_string();

    let version = match run(program, &["--version"]).await {
        Ok(out) if out.ok => out.stdout.trim().to_string(),
        // A non-zero `--version` is not a normal state for any working install.
        Ok(out) => {
            return Readiness::Unknown {
                detail: format!("`{program} --version` failed: {}", first_line(&out.stderr)),
            }
        }
        Err(NotRunnable::Missing) => {
            return Readiness::NotInstalled {
                program: program.to_string(),
            }
        }
        Err(NotRunnable::Failed(detail)) => return Readiness::Unknown { detail },
    };

    // Deliberately no minimum version. The flags this app depends on were verified against
    // 2.1.153, but nobody knows which future release breaks them, and refusing to start on a
    // version we merely have not seen would block working installs on a guess. The version is
    // recorded instead, so a bug report says which one produced it.

    let status = match run(program, &["auth", "status"]).await {
        Ok(out) if out.ok => out.stdout,
        // Non-zero here reliably means "not logged in" rather than a broken install, since the
        // command itself clearly exists.
        Ok(_) => return Readiness::NotAuthenticated { version },
        Err(NotRunnable::Missing) => {
            return Readiness::NotInstalled {
                program: program.to_string(),
            }
        }
        Err(NotRunnable::Failed(detail)) => return Readiness::Unknown { detail },
    };

    let parsed: AuthStatus = match serde_json::from_str(&status) {
        Ok(parsed) => parsed,
        // Output we cannot read is not evidence of being logged out. Saying so would send the
        // operator to re-run a login that already succeeded.
        Err(e) => {
            return Readiness::Unknown {
                detail: format!("could not read `{program} auth status`: {e}"),
            }
        }
    };

    if !parsed.logged_in {
        return Readiness::NotAuthenticated { version };
    }

    Readiness::Ready {
        version,
        path: located,
        auth: AuthInfo {
            method: parsed.auth_method,
            email: parsed.email,
            subscription: parsed.subscription_type,
            organization: parsed.org_name,
        },
    }
}

enum NotRunnable {
    /// The binary is not on PATH.
    Missing,
    Failed(String),
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

async fn run(program: &str, args: &[&str]) -> Result<Output, NotRunnable> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // The same stripped environment agents get. Probing with the developer's full environment
    // could pass while the real spawn fails — the probe would then be reassuring rather than
    // informative, which is worse than not having one.
    cmd.env_clear();
    for key in ENV_ALLOWLIST {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }

    let output = match tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => return Err(NotRunnable::Missing),
        Ok(Err(e)) => return Err(NotRunnable::Failed(format!("could not run {program}: {e}"))),
        Err(_) => {
            return Err(NotRunnable::Failed(format!(
                "`{program} {}` did not answer within {}s",
                args.join(" "),
                PROBE_TIMEOUT.as_secs()
            )))
        }
    };

    Ok(Output {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured verbatim from `claude auth status` v2.1.153.
    const REAL_OUTPUT: &str = r#"{
      "loggedIn": true,
      "authMethod": "claude.ai",
      "apiProvider": "firstParty",
      "email": "someone@example.com",
      "orgId": "7540775e-f927-4253-b535-77231a6d76e6",
      "orgName": "Technance",
      "subscriptionType": "team"
    }"#;

    #[test]
    fn the_real_auth_output_parses_with_its_fields_intact() {
        // The CLI emits camelCase. Without the rename every field would fall back to its
        // default and a logged-in operator would be told to log in again.
        let parsed: AuthStatus = serde_json::from_str(REAL_OUTPUT).expect("parse");
        assert!(parsed.logged_in);
        assert_eq!(parsed.auth_method.as_deref(), Some("claude.ai"));
        assert_eq!(parsed.subscription_type.as_deref(), Some("team"));
        assert_eq!(parsed.org_name.as_deref(), Some("Technance"));
    }

    #[test]
    fn an_unrecognised_field_does_not_break_the_check() {
        // This decides whether the app is usable at all, so a field added by a future CLI must
        // not be able to lock someone out of their own install.
        let parsed: AuthStatus =
            serde_json::from_str(r#"{"loggedIn": true, "somethingNew": 42}"#).expect("parse");
        assert!(parsed.logged_in);
    }

    #[test]
    fn logged_out_is_reported_rather_than_assumed_ready() {
        let parsed: AuthStatus = serde_json::from_str(r#"{"loggedIn": false}"#).expect("parse");
        assert!(!parsed.logged_in);
    }

    #[tokio::test]
    async fn a_missing_binary_is_reported_as_not_installed() {
        // Distinct from every other failure: it is the only one with an instruction the operator
        // can act on without knowing anything about AgentDeck.
        let readiness = probe("definitely-not-a-real-binary-agentdeck").await;
        assert!(
            matches!(readiness, Readiness::NotInstalled { .. }),
            "got {readiness:?}"
        );
    }

    #[tokio::test]
    async fn a_binary_that_is_not_the_cli_is_unknown_rather_than_missing() {
        // Saying "not installed" would send someone to reinstall something that is already
        // there. `false` exists and exits non-zero, standing in for a broken install.
        let readiness = probe("false").await;
        assert!(
            matches!(readiness, Readiness::Unknown { .. }),
            "got {readiness:?}"
        );
    }
}
