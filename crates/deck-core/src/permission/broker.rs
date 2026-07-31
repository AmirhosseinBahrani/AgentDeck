//! Answers `can_use_tool` requests.
//!
//! Allow and deny are settled in-process in microseconds, which is what keeps an autonomous
//! agent feeling fast — the human is involved only where the policy genuinely cannot decide.
//! An `Ask` parks the request until a human answers or the deadline passes.

use crate::permission::policy::{Decision, EffectivePolicy, Rationale};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a pending request waits before being auto-denied.
///
/// The timeout exists so a forgotten prompt cannot wedge an agent forever. It auto-*denies*
/// rather than auto-allowing, and the denial carries an explanation — a silent timeout reads to
/// the model as a tool malfunction and it retries the identical call.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    Allowed { updated_input: Value },
    Denied { message: String },
}

/// A request waiting on a human.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub request_id: String,
    pub tool: String,
    pub input: Value,
    pub reason: String,
    /// The CLI's own classification, e.g. `workingDir`. Present because the CLI tells us *why*
    /// it is asking, so the broker never has to re-derive containment.
    pub cli_reason_type: Option<String>,
    pub blocked_path: Option<String>,
    /// Structured options from the CLI (`addRules` / `addDirectories`), passed to the UI as
    /// typed choices so the frontend never parses prose.
    pub suggestions: Vec<Value>,
    /// Wall-clock deadline, so the UI can show a countdown. A silent auto-deny is the worst
    /// possible failure mode here.
    pub expires_at_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("no pending permission request with id {0}")]
    Unknown(String),
    #[error("permission request {0} was already resolved")]
    AlreadyResolved(String),
}

/// Outcome of handing a tool call to the broker.
pub enum Verdict {
    /// Settled by policy; answer the CLI immediately.
    Immediate(Resolution),
    /// Needs a human. Surface `request` to the UI and await `wait`.
    Escalated {
        request: PendingRequest,
        wait: oneshot::Receiver<Resolution>,
    },
}

pub struct PermissionBroker {
    policy: EffectivePolicy,
    /// Parked responders. `DashMap` so `resolve` can be called from a UI command handler
    /// without contending with in-flight decisions.
    pending: DashMap<String, oneshot::Sender<Resolution>>,
    now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    timeout: Duration,
}

impl PermissionBroker {
    pub fn new(policy: EffectivePolicy) -> Self {
        Self::with_config(
            policy,
            ASK_TIMEOUT,
            Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or_default()
            }),
        )
    }

    pub fn with_config(
        policy: EffectivePolicy,
        timeout: Duration,
        now_ms: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Self {
            policy,
            pending: DashMap::new(),
            now_ms,
            timeout,
        }
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Classifies a tool call. Returns immediately for allow/deny.
    pub fn evaluate(
        &self,
        request_id: &str,
        tool: &str,
        input: &Value,
        cli_reason_type: Option<String>,
        blocked_path: Option<String>,
        suggestions: Vec<Value>,
    ) -> (Verdict, Rationale) {
        let (decision, rationale) = self.policy.decide(tool, input);

        match decision {
            Decision::Allow { updated_input } => (
                Verdict::Immediate(Resolution::Allowed { updated_input }),
                rationale,
            ),
            Decision::Deny { message } => (
                Verdict::Immediate(Resolution::Denied { message }),
                rationale,
            ),
            Decision::Ask { reason } => {
                let (tx, rx) = oneshot::channel();
                self.pending.insert(request_id.to_string(), tx);

                let request = PendingRequest {
                    request_id: request_id.to_string(),
                    tool: tool.to_string(),
                    input: input.clone(),
                    reason,
                    cli_reason_type,
                    blocked_path,
                    suggestions,
                    expires_at_ms: (self.now_ms)() + self.timeout.as_millis() as i64,
                };

                (Verdict::Escalated { request, wait: rx }, rationale)
            }
        }
    }

    /// Applies a human decision. Idempotent from the caller's perspective: answering twice
    /// reports `AlreadyResolved` rather than panicking, since a double-click is expected.
    pub fn resolve(&self, request_id: &str, resolution: Resolution) -> Result<(), BrokerError> {
        let (_, tx) = self
            .pending
            .remove(request_id)
            .ok_or_else(|| BrokerError::Unknown(request_id.to_string()))?;

        tx.send(resolution)
            .map_err(|_| BrokerError::AlreadyResolved(request_id.to_string()))
    }

    /// Auto-denial used when the deadline passes.
    pub fn expire(&self, request_id: &str) {
        let _ = self.resolve(
            request_id,
            Resolution::Denied {
                message: "No response from the operator within the approval window, so this \
                          action was declined. Continue with an approach that stays inside your \
                          worktree, or report a blocker."
                    .into(),
            },
        );
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn is_pending(&self, request_id: &str) -> bool {
        self.pending.contains_key(request_id)
    }

    pub fn policy(&self) -> &EffectivePolicy {
        &self.policy
    }
}

/// Waits for a human answer, falling back to an explained denial at the deadline.
pub async fn await_resolution(
    broker: Arc<PermissionBroker>,
    request_id: String,
    wait: oneshot::Receiver<Resolution>,
) -> Resolution {
    match tokio::time::timeout(broker.timeout(), wait).await {
        Ok(Ok(resolution)) => resolution,
        // Sender dropped without answering — treat as a decline rather than hanging the agent.
        Ok(Err(_)) => Resolution::Denied {
            message: "The approval request was cancelled.".into(),
        },
        Err(_) => {
            broker.expire(&request_id);
            Resolution::Denied {
                message: "No response from the operator within the approval window, so this \
                          action was declined. Continue with an approach that stays inside your \
                          worktree, or report a blocker."
                    .into(),
            }
        }
    }
}
