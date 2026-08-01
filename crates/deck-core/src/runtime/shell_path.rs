//! Recovering the PATH a GUI launch throws away.
//!
//! A `.app` opened from Finder or the Dock does not inherit a shell, so macOS hands it a bare
//! `/usr/bin:/bin:/usr/sbin:/sbin`. Homebrew, nvm, npm's global prefix, cargo and pyenv are all
//! absent. The same binary run from a terminal sees all of them, which is why this is invisible
//! during development and breaks the moment anyone double-clicks the icon.
//!
//! For this app that is not cosmetic. `claude` typically lives in `/opt/homebrew/bin` or an npm
//! prefix, so it simply cannot be found — and even once it is, the CLI shells out to `git`,
//! `node` and a test runner, all of which have to be on the PATH the agent inherits. Fixing this
//! for the readiness probe alone would produce the worst outcome available: a green setup screen
//! in front of agents that cannot run.
//!
//! So the repair is process-wide and happens once, before anything spawns. Every later
//! consumer — the probe, agent spawning, the planner, git — reads the same corrected value.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// Login shells source the user's profile, which is where PATH is actually assembled. That also
/// means arbitrary user code runs, so it gets a deadline.
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);

/// Marks the answer inside whatever else a profile decides to print. Plenty of shell profiles
/// emit banners, version managers, or fortune quotes; without a marker the first line of that
/// noise would be adopted as the PATH.
const MARKER: &str = "__agentdeck_path__";

/// Ensures `tool` is findable, repairing PATH from the user's login shell if it is not.
///
/// Returns the resolved absolute path when one was found. A `None` means the tool genuinely is
/// not installed, which the readiness check then reports honestly rather than as a PATH problem.
///
/// Deliberately conditional: when the app is started from a terminal the PATH is already correct,
/// and spawning a login shell would add startup latency and run the user's profile for nothing.
pub async fn ensure_tool_on_path(tool: &str) -> Option<PathBuf> {
    if let Some(found) = which(tool) {
        return Some(found);
    }

    let recovered = login_shell_path().await?;
    // Adopted wholesale rather than merged: the login shell's PATH is the user's real
    // environment, and its ordering is meaningful — a version manager earlier in the list is how
    // someone selects which node or python they meant.
    std::env::set_var("PATH", &recovered);

    let found = which(tool);
    tracing::info!(
        recovered = %recovered.len(),
        found = found.is_some(),
        "PATH was missing {tool}; adopted the login shell's PATH"
    );
    found
}

/// Asks the user's login shell what its PATH is.
async fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());

    // `-l` sources the login profile, `-i` the interactive one. Which file exports PATH differs
    // per person and per shell, so both are needed to match what they see in a terminal.
    let mut cmd = tokio::process::Command::new(&shell);
    cmd.args(["-lic", &format!("printf '%s%s' {MARKER} \"$PATH\"")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(SHELL_TIMEOUT, cmd.output()).await {
        Ok(Ok(output)) => output,
        // A profile that fails or hangs is the user's, not ours to fix. Startup continues with
        // the PATH we have, and the readiness check reports what it can actually see.
        Ok(Err(e)) => {
            tracing::warn!(%e, "could not ask {shell} for its PATH");
            return None;
        }
        Err(_) => {
            tracing::warn!("{shell} did not report its PATH within {SHELL_TIMEOUT:?}");
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value = stdout.rsplit(MARKER).next()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Resolves an executable against the current PATH.
///
/// Hand-rolled rather than shelling out to `which`, because `which` is itself resolved through
/// the PATH being diagnosed — and on the bare GUI PATH that is exactly the situation.
fn which(tool: &str) -> Option<PathBuf> {
    // An explicit path is already an answer, and must not be searched for.
    if tool.contains(std::path::MAIN_SEPARATOR) {
        let candidate = PathBuf::from(tool);
        return is_executable(&candidate).then_some(candidate);
    }

    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(tool))
            .find(|candidate| is_executable(candidate))
    })
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_path_is_returned_without_searching() {
        assert_eq!(which("/bin/sh"), Some(PathBuf::from("/bin/sh")));
    }

    #[test]
    fn a_missing_tool_resolves_to_nothing() {
        assert_eq!(which("definitely-not-a-real-binary-agentdeck"), None);
    }

    #[test]
    fn a_directory_is_not_mistaken_for_an_executable() {
        // `/bin` is present on every PATH and is executable-by-mode, so a naive check that only
        // looked at permission bits would happily return a directory.
        assert!(!is_executable(std::path::Path::new("/bin")));
    }

    #[tokio::test]
    async fn a_tool_already_on_path_is_found_without_consulting_a_shell() {
        // The common case, and the one that must stay fast: started from a terminal, nothing to
        // repair, no profile executed.
        assert!(ensure_tool_on_path("sh").await.is_some());
    }

    #[tokio::test]
    async fn the_login_shell_reports_a_usable_path() {
        // Guards the marker parsing against profiles that print banners: whatever else the shell
        // emits, what comes back has to look like a PATH.
        let Some(path) = login_shell_path().await else {
            return; // No usable login shell in this environment; nothing to assert.
        };
        assert!(!path.contains(MARKER), "the marker must be stripped");
        assert!(
            path.split(':').any(|dir| dir.starts_with('/')),
            "expected absolute directories, got {path:?}"
        );
    }
}
