//! One actor per session, owning the child process.
//!
//! Ordering constraints here are load-bearing and easy to break:
//!
//! - `stdin` is taken out of the `Child` before anything polls `wait()`. `wait()` drops
//!   stdin, and dropping stdin ends the session — so polling it in the same `select!` while
//!   still holding stdin would silently terminate a healthy agent.
//! - stderr is drained by its own task and never awaited in the main `select!`. An unread
//!   stderr pipe fills and deadlocks the child.
//! - Force-kill does not go through this actor. The process group handle lives in the
//!   manager, because a wedged actor is exactly what needs killing.

use crate::bus::{Attribution, DeltaChannel, EventBus};
use crate::domain::event::{AgentEvent, ExitReason};
use crate::domain::ids::SessionId;
use crate::permission::broker::{await_resolution, PermissionBroker, Resolution, Verdict};
use crate::process::{self, PlatformProcessGroup, ProcessGroup};
use crate::runtime::claude_code::argv::{PermissionMode, SessionConfig};
use crate::runtime::claude_code::ndjson::{Line, NdjsonReader};
use crate::runtime::claude_code::translate::Translator;
use crate::runtime::claude_code::wire::{Inbound, PermissionDecision};
use crate::runtime::{Result, RuntimeError};
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

/// How long to wait for the first `system/init` before declaring startup failed. Without
/// this, an auth failure or an unsupported CLI version presents as an indefinite hang.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const STDERR_TAIL_LINES: usize = 256;

#[derive(Debug)]
pub enum SessionCmd {
    SendText(String),
    RespondPermission {
        request_id: String,
        decision: PermissionDecision,
    },
    SetPermissionMode(PermissionMode),
    /// Cooperative: asks the agent to stop the current turn so it can still report.
    Interrupt,
    /// Close stdin and let the process exit on its own.
    Shutdown,
}

pub struct SessionHandle {
    pub session_id: SessionId,
    cmd_tx: mpsc::Sender<SessionCmd>,
    /// Held here, outside the actor, so `kill_now` works even if the actor is stuck.
    group: Arc<PlatformProcessGroup>,
    cancel: CancellationToken,
    deltas: DeltaChannel,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl SessionHandle {
    pub async fn send(&self, cmd: SessionCmd) -> Result<()> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| RuntimeError::NotWritable(self.session_id))
    }

    /// Immediate, unconditional termination of the whole process tree.
    ///
    /// Intentionally synchronous and lock-free: it must not await the actor, the control
    /// channel, or the NDJSON parser, since any of those may be the thing that is stuck.
    pub fn kill_now(&self) -> Result<()> {
        self.group.kill_now()?;
        self.cancel.cancel();
        Ok(())
    }

    pub fn is_alive(&self) -> bool {
        self.group.is_alive()
    }

    pub fn pid(&self) -> u32 {
        self.group.pid()
    }

    pub fn deltas(&self) -> &DeltaChannel {
        &self.deltas
    }

    pub async fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().await.iter().cloned().collect()
    }
}

pub struct SpawnOptions {
    pub program: String,
    pub config: SessionConfig,
    pub attribution: Attribution,
    /// Extra environment on top of the inherited allowlist.
    pub env: Vec<(String, String)>,
    /// How long to wait for the first `system/init`. Configurable so tests can assert the
    /// real startup-failure path without waiting out the production timeout.
    pub startup_timeout: Duration,
    /// Answers `can_use_tool` requests. Without one, the CLI's own `acceptEdits` handling
    /// applies and out-of-worktree actions are refused with no chance to approve them.
    pub broker: Option<Arc<PermissionBroker>>,
}

impl SpawnOptions {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            program: "claude".into(),
            config,
            attribution: Attribution::default(),
            env: Vec::new(),
            startup_timeout: STARTUP_TIMEOUT,
            broker: None,
        }
    }
}

/// Environment the CLI genuinely needs. Everything else is stripped so an agent cannot
/// inherit GITHUB_TOKEN, AWS_*, npm tokens and similar from the developer's shell.
///
/// Note this must not be empty: OAuth credentials come from the OS keychain, which needs
/// HOME, and the CLI shells out, which needs PATH.
pub const ENV_ALLOWLIST: &[&str] = &[
    "HOME", "PATH", "USER", "LOGNAME", "SHELL", "TMPDIR", "LANG", "LC_ALL", "TERM", "TZ",
];

pub async fn spawn_session(
    opts: SpawnOptions,
    bus: Arc<EventBus>,
) -> Result<(SessionHandle, tokio::task::JoinHandle<ExitReason>)> {
    let session_id = opts.config.session_id;
    let mut cmd = tokio::process::Command::new(&opts.program);
    cmd.args(opts.config.to_argv())
        .current_dir(&opts.config.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    cmd.env_clear();
    for key in ENV_ALLOWLIST {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }

    process::configure_group(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| RuntimeError::Spawn(format!("{}: {e}", opts.program)))?;

    let group = Arc::new(process::adopt(&child)?);

    // Must happen before anything can poll `wait()`.
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| RuntimeError::Spawn("no stdin pipe".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RuntimeError::Spawn("no stdout pipe".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RuntimeError::Spawn("no stderr pipe".into()))?;

    // Shared copy for the handle to read. Written by the actor, not by the reader task, so
    // the actor's own view is always current when it classifies a startup failure.
    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));

    // The pipe gets its own task so draining is never gated on the main loop's progress —
    // an unread stderr pipe fills and deadlocks the child. Lines are handed to the actor
    // rather than stored here, so classification does not depend on task scheduling.
    let (stderr_tx, stderr_rx) = mpsc::channel::<String>(512);
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // try_send, not send: if the actor is busy, keep draining the pipe and drop
            // diagnostics rather than risk stalling the child.
            if stderr_tx.try_send(line).is_err() {
                continue;
            }
        }
    });

    let (cmd_tx, cmd_rx) = mpsc::channel(32);
    let cancel = CancellationToken::new();
    let deltas = DeltaChannel::new();

    let actor = Actor {
        session_id,
        startup_timeout: opts.startup_timeout,
        broker: opts.broker.clone(),
        self_tx: cmd_tx.clone(),
        attribution: opts.attribution,
        bus,
        translator: Translator::new(),
        deltas: deltas.clone(),
        stderr_tail: stderr_tail.clone(),
        stderr_rx,
        local_tail: VecDeque::with_capacity(STDERR_TAIL_LINES),
        cancel: cancel.clone(),
        started: false,
    };

    let join = tokio::spawn(actor.run(child, stdin, stdout, cmd_rx));

    Ok((
        SessionHandle {
            session_id,
            cmd_tx,
            group,
            cancel,
            deltas,
            stderr_tail,
        },
        join,
    ))
}

struct Actor {
    session_id: SessionId,
    startup_timeout: Duration,
    broker: Option<Arc<PermissionBroker>>,
    /// The actor's own command channel. Permission answers are posted back through it rather
    /// than reaching into stdin directly, so all writes stay serialized in one place.
    self_tx: mpsc::Sender<SessionCmd>,
    attribution: Attribution,
    bus: Arc<EventBus>,
    translator: Translator,
    deltas: DeltaChannel,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_rx: mpsc::Receiver<String>,
    /// The actor's own view of stderr, always current at classification time.
    local_tail: VecDeque<String>,
    cancel: CancellationToken,
    started: bool,
}

impl Actor {
    async fn run(
        mut self,
        mut child: tokio::process::Child,
        mut stdin: tokio::process::ChildStdin,
        stdout: tokio::process::ChildStdout,
        mut cmd_rx: mpsc::Receiver<SessionCmd>,
    ) -> ExitReason {
        let mut reader = NdjsonReader::new(BufReader::new(stdout));
        let mut interrupted = false;
        let startup_deadline = tokio::time::sleep(self.startup_timeout);
        tokio::pin!(startup_deadline);

        let reason = loop {
            tokio::select! {
                biased;

                _ = self.cancel.cancelled() => {
                    break ExitReason::Killed;
                }

                // Only armed until startup completes, so a long-running healthy session is
                // never killed by the startup timer.
                _ = &mut startup_deadline, if !self.started => {
                    // Let the stderr drain task catch up before classifying. Reading the tail
                    // the instant the timer fires can race the drain and report "no error
                    // output" while the CLI's actual explanation is still in flight — which
                    // would turn an actionable auth error into a mystery timeout.
                    let tail = self.drain_pending_stderr().await;
                    let detail = classify_startup_failure(&tail);
                    self.emit(AgentEvent::SessionExited {
                        reason: ExitReason::StartupFailed { detail: detail.clone() },
                    }).await;
                    return ExitReason::StartupFailed { detail };
                }

                Some(line) = self.stderr_rx.recv() => {
                    self.record_stderr(line).await;
                }

                Some(cmd) = cmd_rx.recv() => {
                    if let Some(r) = self.handle_cmd(cmd, &mut stdin, &mut interrupted).await {
                        break r;
                    }
                }

                line = reader.next_line() => {
                    match line {
                        Ok(Line::Eof) => break self.on_exit(&mut child, interrupted).await,
                        Ok(Line::Json(text)) => {
                            if !text.is_empty() {
                                self.on_line(&text).await;
                            }
                        }
                        Ok(Line::Oversized { bytes }) => {
                            self.emit(AgentEvent::Diagnostic {
                                message: format!(
                                    "dropped an oversized stream line ({bytes} bytes); \
                                     stream resynchronized"
                                ),
                            }).await;
                        }
                        Err(e) => {
                            self.emit(AgentEvent::Diagnostic {
                                message: format!("stdout read error: {e}"),
                            }).await;
                            break self.on_exit(&mut child, interrupted).await;
                        }
                    }
                }

                status = child.wait() => {
                    break match status {
                        Ok(s) if s.success() => ExitReason::Clean,
                        Ok(s) => ExitReason::Crashed { code: s.code() },
                        Err(e) => ExitReason::StartupFailed { detail: e.to_string() },
                    };
                }
            }
        };

        self.emit(AgentEvent::SessionExited {
            reason: reason.clone(),
        })
        .await;
        reason
    }

    /// Returns `Some(reason)` when the command ends the session.
    async fn handle_cmd(
        &mut self,
        cmd: SessionCmd,
        stdin: &mut tokio::process::ChildStdin,
        interrupted: &mut bool,
    ) -> Option<ExitReason> {
        let payload = match cmd {
            SessionCmd::SendText(text) => Some(Inbound::user_text(text)),
            SessionCmd::RespondPermission {
                request_id,
                decision,
            } => {
                let allowed = matches!(decision, PermissionDecision::Allow { .. });
                self.emit(AgentEvent::PermissionResolved {
                    request_id: request_id.clone(),
                    allowed,
                })
                .await;
                Some(Inbound::permission(request_id, decision))
            }
            SessionCmd::SetPermissionMode(mode) => Some(Inbound::ControlRequest {
                request_id: uuid::Uuid::new_v4().to_string(),
                request: serde_json::json!({
                    "subtype": "set_permission_mode",
                    "mode": mode.as_flag(),
                }),
            }),
            SessionCmd::Interrupt => {
                *interrupted = true;
                Some(Inbound::ControlRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    request: serde_json::json!({ "subtype": "interrupt" }),
                })
            }
            SessionCmd::Shutdown => None,
        };

        match payload {
            Some(msg) => {
                match msg.to_ndjson_line() {
                    Ok(line) => {
                        if stdin.write_all(line.as_bytes()).await.is_err()
                            || stdin.flush().await.is_err()
                        {
                            // The child closed stdin; it is on its way out.
                            return Some(ExitReason::Clean);
                        }
                    }
                    Err(e) => {
                        self.emit(AgentEvent::Diagnostic {
                            message: format!("failed to serialize input: {e}"),
                        })
                        .await;
                    }
                }
                None
            }
            None => {
                // Closing stdin is the documented graceful stop for a duplex session.
                let _ = stdin.shutdown().await;
                None
            }
        }
    }

    async fn on_line(&mut self, text: &str) {
        // Token deltas bypass the bus entirely and are dropped when unobserved.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            if v.get("type").and_then(|t| t.as_str()) == Some("stream_event") {
                if self.deltas.has_subscribers() {
                    if let Some(chunk) = extract_text_delta(&v) {
                        self.deltas.push(chunk);
                    }
                }
                return;
            }
        }

        for event in self.translator.translate(text) {
            if matches!(event, AgentEvent::SessionReady { .. }) {
                self.started = true;
            }

            if let AgentEvent::PermissionRequest {
                ref request_id,
                ref tool,
                ref input,
                ref reason_type,
                ref blocked_path,
                ref suggestions,
            } = event
            {
                self.handle_permission_request(
                    request_id.clone(),
                    tool.clone(),
                    input.clone(),
                    reason_type.clone(),
                    blocked_path.clone(),
                    suggestions.clone(),
                )
                .await;
            }

            self.emit(event).await;
        }
    }

    /// Routes a `can_use_tool` request through the broker.
    ///
    /// Policy-settled outcomes are answered on the spot, so routine work never waits on a
    /// human. Escalations are answered by a detached task once the operator responds or the
    /// deadline passes — the actor must not block here, or the agent's other output would stop
    /// being read while a prompt sits unanswered.
    #[allow(clippy::too_many_arguments)]
    async fn handle_permission_request(
        &mut self,
        request_id: String,
        tool: String,
        input: serde_json::Value,
        reason_type: Option<String>,
        blocked_path: Option<String>,
        suggestions: Vec<serde_json::Value>,
    ) {
        let Some(broker) = self.broker.clone() else {
            return;
        };

        let (verdict, rationale) = broker.evaluate(
            &request_id,
            &tool,
            &input,
            reason_type,
            blocked_path,
            suggestions,
        );

        match verdict {
            Verdict::Immediate(resolution) => {
                tracing::debug!(?rationale, tool = %tool, "permission settled by policy");
                self.answer_permission(request_id, resolution).await;
            }
            Verdict::Escalated { request, wait } => {
                tracing::info!(?rationale, tool = %tool, "permission escalated to operator");
                let tx = self.self_tx.clone();
                let broker = broker.clone();
                let id = request.request_id.clone();
                tokio::spawn(async move {
                    let resolution = await_resolution(broker, id.clone(), wait).await;
                    let _ = tx
                        .send(SessionCmd::RespondPermission {
                            request_id: id,
                            decision: to_wire_decision(resolution),
                        })
                        .await;
                });
            }
        }
    }

    async fn answer_permission(&self, request_id: String, resolution: Resolution) {
        let _ = self
            .self_tx
            .send(SessionCmd::RespondPermission {
                request_id,
                decision: to_wire_decision(resolution),
            })
            .await;
    }

    async fn on_exit(
        &mut self,
        child: &mut tokio::process::Child,
        interrupted: bool,
    ) -> ExitReason {
        // stdout closed. Reap so the exit status is accurate rather than inferred.
        match child.wait().await {
            Ok(s) if s.success() => ExitReason::Clean,
            Ok(_) if interrupted => ExitReason::Interrupted,
            Ok(s) => ExitReason::Crashed { code: s.code() },
            Err(e) => ExitReason::Crashed {
                code: e.raw_os_error(),
            },
        }
    }

    async fn record_stderr(&mut self, line: String) {
        if self.local_tail.len() == STDERR_TAIL_LINES {
            self.local_tail.pop_front();
        }
        self.local_tail.push_back(line);
        *self.stderr_tail.lock().await = self.local_tail.clone();
    }

    /// Collects any stderr still in flight, then returns the actor's view.
    ///
    /// A startup failure must be explained using what the CLI actually printed. Classifying
    /// against a stale view reports "no error output" while the real explanation is still
    /// queued, which turns an actionable auth error into a mystery timeout.
    async fn drain_pending_stderr(&mut self) -> Vec<String> {
        // Wait briefly for the first line if nothing has arrived yet, then take whatever
        // else is immediately available.
        if self.local_tail.is_empty() {
            if let Ok(Some(line)) =
                tokio::time::timeout(Duration::from_millis(750), self.stderr_rx.recv()).await
            {
                self.record_stderr(line).await;
            }
        }
        while let Ok(line) = self.stderr_rx.try_recv() {
            self.record_stderr(line).await;
        }
        self.local_tail.iter().cloned().collect()
    }

    async fn emit(&self, event: AgentEvent) {
        let mut attribution = self.attribution;
        attribution.session_id = Some(self.session_id);
        self.bus.publish(attribution, event).await;
    }
}

fn to_wire_decision(resolution: Resolution) -> PermissionDecision {
    match resolution {
        Resolution::Allowed { updated_input } => PermissionDecision::Allow { updated_input },
        Resolution::Denied { message } => PermissionDecision::Deny { message },
    }
}

fn extract_text_delta(v: &serde_json::Value) -> Option<String> {
    v.get("event")?
        .get("delta")?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

/// Turns stderr noise into something a user can act on. Without this, the three most common
/// setup failures all present identically as "the agent never started".
fn classify_startup_failure(stderr_tail: &[String]) -> String {
    let joined = stderr_tail.join("\n");
    let lower = joined.to_lowercase();

    if lower.contains("unknown option") || lower.contains("unknown argument") {
        return format!(
            "The installed Claude Code does not recognize a flag AgentDeck relies on, so the \
             CLI is likely older than supported. Details: {joined}"
        );
    }
    if lower.contains("auth") || lower.contains("login") || lower.contains("credential") {
        return format!(
            "Claude Code is not authenticated — try `claude auth login`. Details: {joined}"
        );
    }
    if lower.contains("requires --verbose") {
        return "stream-json output requires --verbose; this is an AgentDeck bug in argv construction."
            .to_string();
    }
    if joined.trim().is_empty() {
        return "The agent produced no startup handshake and no error output within the timeout."
            .to_string();
    }
    format!("The agent failed to start. Details: {joined}")
}
