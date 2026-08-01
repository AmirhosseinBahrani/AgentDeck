//! Working out how a repository tests itself.
//!
//! The supervisor needs one command it can run to decide whether the integrated work is sound,
//! and it needs it before any agent has been dispatched. Asking a model would be both a
//! round-trip and a guess; the answer is sitting in the repository root in the form of whichever
//! manifest the project's toolchain uses.

use std::path::Path;

/// The command the supervisor will run to judge a repository, and how it was arrived at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestCommand {
    /// A manifest at the repository root identified the toolchain.
    Detected { cmd: String, from: &'static str },
    /// Nothing recognisable. Deliberately not a guess — see [`detect_test_command`].
    Unknown,
}

impl TestCommand {
    /// The command to run, or `None` when the project's toolchain could not be identified.
    pub fn cmd(&self) -> Option<&str> {
        match self {
            TestCommand::Detected { cmd, .. } => Some(cmd),
            TestCommand::Unknown => None,
        }
    }
}

/// Identifies a repository's test command from the manifests at its root.
///
/// Returns [`TestCommand::Unknown`] rather than a plausible default when nothing matches. A
/// fabricated command does not fail as "we could not tell what this project is" — it fails as a
/// red test, which the integration gate is obliged to treat as broken work. Every run against an
/// empty or unrecognised repository would then be unable to complete, for a reason nothing in the
/// UI could explain. Callers decide what to do with the absence; this function will not invent one.
pub fn detect_test_command(root: &Path) -> TestCommand {
    let has = |name: &str| root.join(name).exists();

    let detected = |cmd: &str, from: &'static str| TestCommand::Detected {
        cmd: cmd.to_string(),
        from,
    };

    if has("Cargo.toml") {
        return detected("cargo test", "Cargo.toml");
    }
    if has("go.mod") {
        return detected("go test ./...", "go.mod");
    }
    if has("package.json") {
        // Only when a `test` script exists. npm answers an undefined script with a non-zero exit,
        // which would read to the gate as a failing test suite rather than a missing one.
        if let Some(cmd) = node_test_command(root) {
            return TestCommand::Detected {
                cmd,
                from: "package.json",
            };
        }
    }
    if has("pyproject.toml") || has("pytest.ini") || has("tox.ini") || has("setup.cfg") {
        return detected("pytest", "pyproject.toml");
    }
    if has("pom.xml") {
        return detected("mvn -q test", "pom.xml");
    }
    if has("build.gradle") || has("build.gradle.kts") {
        return detected("./gradlew test", "build.gradle");
    }
    if has("mix.exs") {
        return detected("mix test", "mix.exs");
    }
    if has("Gemfile") && root.join("spec").is_dir() {
        return detected("bundle exec rspec", "Gemfile");
    }
    if makefile_has_test_target(root) {
        return detected("make test", "Makefile");
    }

    TestCommand::Unknown
}

/// The package manager's test script, if `package.json` declares one.
fn node_test_command(root: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(root.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    parsed.get("scripts")?.get("test")?.as_str()?;

    // Taken from the lockfile rather than the manifest: `packageManager` is frequently absent,
    // while a committed lockfile is what the project actually installs with.
    let runner = if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lockb").exists() {
        "bun"
    } else {
        "npm"
    };

    Some(format!("{runner} test"))
}

/// Whether a root Makefile declares a `test` target.
fn makefile_has_test_target(root: &Path) -> bool {
    let Ok(makefile) = std::fs::read_to_string(root.join("Makefile")) else {
        return false;
    };
    makefile
        .lines()
        .any(|line| line.starts_with("test:") || line.starts_with("test :"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("deck-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_repository_yields_no_command() {
        // The case that prompted this module. A freshly initialised project used to be handed
        // `cargo test`, so the integration gate failed with cargo's own "could not find
        // Cargo.toml" — an error about the supervisor's assumption, presented as the project's
        // tests failing.
        assert_eq!(detect_test_command(&scratch("empty")), TestCommand::Unknown);
    }

    #[test]
    fn rust_and_go_are_read_from_their_manifests() {
        let dir = scratch("rust");
        std::fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(detect_test_command(&dir).cmd(), Some("cargo test"));

        let dir = scratch("go");
        std::fs::write(dir.join("go.mod"), "module x\n").unwrap();
        assert_eq!(detect_test_command(&dir).cmd(), Some("go test ./..."));
    }

    #[test]
    fn a_node_project_uses_the_package_manager_its_lockfile_implies() {
        let dir = scratch("node");
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"test":"vitest"}}"#).unwrap();
        assert_eq!(detect_test_command(&dir).cmd(), Some("npm test"));

        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_test_command(&dir).cmd(), Some("pnpm test"));
    }

    #[test]
    fn a_node_project_without_a_test_script_is_not_given_one() {
        // `npm test` on a project with no test script exits non-zero, which the gate would read
        // as a failing suite rather than an absent one.
        let dir = scratch("noscript");
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"build":"vite"}}"#).unwrap();
        assert_eq!(detect_test_command(&dir), TestCommand::Unknown);
    }

    #[test]
    fn a_makefile_counts_only_when_it_declares_a_test_target() {
        let dir = scratch("make");
        std::fs::write(dir.join("Makefile"), "build:\n\tcc main.c\n").unwrap();
        assert_eq!(detect_test_command(&dir), TestCommand::Unknown);

        std::fs::write(
            dir.join("Makefile"),
            "build:\n\tcc main.c\ntest:\n\t./run\n",
        )
        .unwrap();
        assert_eq!(detect_test_command(&dir).cmd(), Some("make test"));
    }
}
