// Mirrors deck-core's serde representation. Hand-written for now; once IPC payload types
// settle, ts-rs generates this and the duplication goes away.

export type ExitReason =
  | "clean"
  | "interrupted"
  | "killed"
  | { crashed: { code: number | null } }
  | { startup_failed: { detail: string } };

export type AgentEvent =
  | { kind: "session_ready"; cwd: string; model: string | null; tools: string[] }
  | { kind: "turn_started" }
  | { kind: "message"; role: string; content: unknown }
  | { kind: "tool_call"; tool_use_id: string; tool: string; input: unknown }
  | { kind: "tool_result"; tool_use_id: string; output: unknown; is_error: boolean }
  | {
      kind: "permission_request";
      request_id: string;
      tool: string;
      input: unknown;
      reason_type: string | null;
      blocked_path: string | null;
      suggestions: unknown[];
    }
  | { kind: "permission_resolved"; request_id: string; allowed: boolean }
  | { kind: "rate_limited"; status: string; resets_at: number | null; limit_type: string | null }
  | {
      kind: "turn_complete";
      subtype: string;
      is_error: boolean;
      cost_usd: number | null;
      structured_output: unknown;
    }
  | { kind: "session_exited"; reason: ExitReason }
  | { kind: "unrecognized"; raw: unknown }
  | { kind: "diagnostic"; message: string };

export interface EventEnvelope {
  seq: number;
  at_ms: number;
  session_id: string | null;
  agent_id: string | null;
  task_id: string | null;
  event: AgentEvent;
}

export interface DeltaChunk {
  session_id: string;
  text: string;
}

export interface EventBatch {
  events: EventEnvelope[];
  deltas: DeltaChunk[];
  first_seq: number | null;
  last_seq: number | null;
}

/** A rendered transcript entry. Deliberately flatter than AgentEvent: the view needs stable,
 *  cheap-to-compare rows, not the full event union. */
export type TranscriptRow =
  | { id: string; type: "text"; role: string; text: string }
  | { id: string; type: "tool_call"; tool: string; input: unknown }
  | { id: string; type: "tool_result"; toolUseId: string; output: string; isError: boolean }
  | { id: string; type: "permission"; requestId: string; tool: string; reason: string | null }
  | { id: string; type: "turn"; costUsd: number | null; isError: boolean }
  | { id: string; type: "notice"; text: string; severity: "info" | "warn" | "error" };

// Mirrors src-tauri's RunSnapshot. The dashboard reads one snapshot rather than several queries,
// so objective, phase, tasks and counts always agree with each other.
export interface TaskSummary {
  id: string;
  title: string;
  status: string;
  role: string;
  attempts: number;
  review_rounds: number;
  objective_gate: boolean;
  blocked_reason: string | null;
  /** Assigned and ready, but held because this mode requires a human to start it. */
  awaiting_approval: boolean;
}

export interface DecisionSummary {
  iteration: number;
  stage: string;
  kind: string;
  /** "code" | "claude" | "human" — whether the model was actually driving. */
  decided_by: string;
  rationale: string;
  repaired: boolean;
}

export interface RunSnapshot {
  active: boolean;
  objective: string;
  phase: string;
  iteration: number;
  spent_usd: number;
  open_escalations: number;
  autonomy: Autonomy;
  /** Whether the branches have been merged and tested together. */
  integrated: boolean;
  tasks: TaskSummary[];
  decisions: DecisionSummary[];
}

/** How much the supervisor may do without being asked. Enforced in Rust at dispatch. */
export type Autonomy = "manual" | "assisted" | "autonomous";

/** What starting up had to clean up after a previous launch. */
export interface RecoveryReport {
  killed_orphans: number;
  stale_records: number;
  interrupted_sessions: number;
  interrupted_runs: number;
}

export interface ResumableSummary {
  session_id: string;
  task_id: string | null;
  /** The directory the session ran in. `--resume` only works from here. */
  cwd: string;
  status: string;
  /** False once the worktree is gone, which makes the conversation unreachable. */
  resumable: boolean;
}

/** Whether the `claude` CLI is installed and logged in. Not an AgentDeck account — there is none. */
export type Readiness =
  | { state: "ready"; version: string; auth: AuthInfo }
  | { state: "not_installed"; program: string }
  | { state: "not_authenticated"; version: string }
  | { state: "unknown"; detail: string };

export interface AuthInfo {
  method: string | null;
  email: string | null;
  /** On subscription billing this, not a dollar budget, is what limits concurrent agents. */
  subscription: string | null;
  organization: string | null;
}
