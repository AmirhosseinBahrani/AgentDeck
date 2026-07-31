//! Pure `SessionConfig` -> argv translation. Kept free of I/O so it can be snapshot-tested
//! without spawning anything; every spike finding about required flags lives here.

use crate::domain::ids::SessionId;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    DontAsk,
    Auto,
    BypassPermissions,
}

impl PermissionMode {
    pub fn as_flag(self) -> &'static str {
        match self {
            PermissionMode::Default => "default",
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::DontAsk => "dontAsk",
            PermissionMode::Auto => "auto",
            PermissionMode::BypassPermissions => "bypassPermissions",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Host-assigned. We never scrape the session id out of the output stream.
    pub session_id: SessionId,
    /// The agent's worktree. Also the containment boundary: no `--add-dir` is passed,
    /// so writes outside this directory land in the permission pipeline.
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub system_prompt_append: Option<String>,
    pub permission_mode: PermissionMode,
    pub tools: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub mcp_config: Option<String>,
    pub json_schema: Option<String>,
    pub max_budget_usd: Option<f64>,
    pub include_partial_messages: bool,
    /// Enables the in-band `can_use_tool` control request. This is exactly what the
    /// TypeScript SDK's `canUseTool` callback compiles down to.
    pub intercept_permissions: bool,
    pub resume: Option<SessionId>,
}

impl SessionConfig {
    pub fn new(session_id: SessionId, cwd: PathBuf) -> Self {
        Self {
            session_id,
            cwd,
            model: None,
            system_prompt_append: None,
            permission_mode: PermissionMode::AcceptEdits,
            tools: Vec::new(),
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            mcp_config: None,
            json_schema: None,
            max_budget_usd: None,
            include_partial_messages: true,
            intercept_permissions: true,
            resume: None,
        }
    }

    pub fn to_argv(&self) -> Vec<OsString> {
        let mut a: Vec<OsString> = Vec::new();
        let mut push = |s: &str| a.push(OsString::from(s));

        push("-p");
        push("--input-format");
        push("stream-json");
        push("--output-format");
        push("stream-json");
        // Mandatory: with --print, stream-json output exits 1 without --verbose.
        push("--verbose");

        // Agents must never inherit the human's plugins, hooks or MCP servers. Besides
        // being nondeterministic and a hook-execution hazard, leaving these on measured
        // 31k vs 8k cache-creation tokens per turn (~4x the cost).
        push("--setting-sources");
        push("");
        push("--strict-mcp-config");

        match &self.resume {
            // Sessions bucket by cwd, so a resume must run in the directory the session
            // was created in; the caller is responsible for that and it is asserted here.
            Some(id) => {
                push("--resume");
                push(&id.to_string());
            }
            None => {
                push("--session-id");
                push(&self.session_id.to_string());
            }
        }

        if let Some(m) = &self.model {
            push("--model");
            push(m);
        }
        push("--permission-mode");
        push(self.permission_mode.as_flag());

        if self.intercept_permissions {
            push("--permission-prompt-tool");
            push("stdio");
        }

        // Always pass --tools, even empty, so the surface is explicit rather than inherited.
        push("--tools");
        push(&self.tools.join(","));

        if !self.allowed_tools.is_empty() {
            push("--allowedTools");
            push(&self.allowed_tools.join(","));
        }
        if !self.disallowed_tools.is_empty() {
            push("--disallowedTools");
            push(&self.disallowed_tools.join(","));
        }
        if let Some(cfg) = &self.mcp_config {
            push("--mcp-config");
            push(cfg);
        }
        if let Some(schema) = &self.json_schema {
            push("--json-schema");
            push(schema);
        }
        if let Some(b) = self.max_budget_usd {
            push("--max-budget-usd");
            push(&b.to_string());
        }
        if let Some(sp) = &self.system_prompt_append {
            push("--append-system-prompt");
            push(sp);
        }
        if self.include_partial_messages {
            push("--include-partial-messages");
        }
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv_of(c: &SessionConfig) -> Vec<String> {
        c.to_argv()
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    fn base() -> SessionConfig {
        SessionConfig::new(SessionId::new(), PathBuf::from("/tmp/wt"))
    }

    #[test]
    fn always_passes_verbose_with_stream_json() {
        let v = argv_of(&base());
        assert!(v.contains(&"--verbose".to_string()));
    }

    #[test]
    fn pins_settings_and_mcp_so_agents_do_not_inherit_user_environment() {
        let v = argv_of(&base());
        let i = v.iter().position(|s| s == "--setting-sources").unwrap();
        assert_eq!(v[i + 1], "");
        assert!(v.contains(&"--strict-mcp-config".to_string()));
    }

    #[test]
    fn permission_interception_maps_to_stdio_prompt_tool() {
        let v = argv_of(&base());
        let i = v.iter().position(|s| s == "--permission-prompt-tool").unwrap();
        assert_eq!(v[i + 1], "stdio");
    }

    #[test]
    fn resume_replaces_session_id_rather_than_accompanying_it() {
        let mut c = base();
        let prior = SessionId::new();
        c.resume = Some(prior);
        let v = argv_of(&c);
        assert!(v.contains(&"--resume".to_string()));
        assert!(!v.contains(&"--session-id".to_string()));
    }

    #[test]
    fn tools_flag_is_always_explicit_even_when_empty() {
        let v = argv_of(&base());
        let i = v.iter().position(|s| s == "--tools").unwrap();
        assert_eq!(v[i + 1], "");
    }
}
