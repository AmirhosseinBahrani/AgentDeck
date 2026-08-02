/** The destinations in the left rail. */
export type NavTab =
  | "team"
  | "workspace"
  | "graph"
  | "diffs"
  | "decisions"
  | "supervisor"
  | "memory"
  | "skills"
  | "files"
  | "advanced";

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
  /** The agent's session, so its transcript is reachable from the task. */
  session_id: string | null;
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
  escalations: Escalation[];
  guidance: Guidance[];
  /** Short run id for the header — enough to tell two runs apart, not the full uuid. */
  run_id: string;
  started_at_ms: number;
  agents: AgentSummary[];
  edges: GraphEdge[];
  max_concurrent: number;
  engaged: number;
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
  | { state: "ready"; version: string; auth: AuthInfo; path: string }
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

/** A typed answer. Never free text — the model must not be able to widen its own permissions. */
export type EscalationAnswer =
  | { action: "retry_planning" }
  | { action: "retry_task"; task_id: string }
  | { action: "abandon_task"; task_id: string }
  | { action: "reintegrate" }
  | { action: "cancel_run" };

export interface EscalationOption {
  label: string;
  /** What choosing it does, in the operator's terms. */
  consequence: string;
  answer: EscalationAnswer;
  destructive: boolean;
}

/** Something the run needs a person to decide before it can continue. */
export interface Escalation {
  id: string;
  kind: string;
  task_id: string | null;
  question: string;
  detail: string;
  options: EscalationOption[];
  opened_at_iteration: number;
}

/** One member of the team, as the roster shows them. */
export interface AgentSummary {
  id: string;
  name: string;
  role: string;
  /** running | blocked | idle */
  status: string;
  activity: string | null;
  task_id: string | null;
  session_id: string | null;
  branch: string | null;
  attempts: number;
  review_rounds: number;
}

/** A named procedure every agent is given at spawn. */
export interface Skill {
  id: string;
  name: string;
  description: string;
  body: string;
  enabled: boolean;
}

/** A session that has already run, listed so its transcript stays readable after the run ends. */
export interface SessionHistoryEntry {
  session_id: string;
  agent_name: string;
  task_title: string | null;
  status: string;
  started_at: number | null;
  ended_at: number | null;
  cost_usd: number;
}

export interface GraphEdge {
  from: string;
  to: string;
  /** "hard" blocks readiness; "soft" only orders the work. */
  kind: string;
}

export interface FileDiff {
  path: string;
  added: number;
  removed: number;
  /** False while the change is still only in the working tree. */
  committed: boolean;
}

export interface TaskDiff {
  task_id: string;
  title: string;
  role: string;
  branch: string | null;
  files: FileDiff[];
  added: number;
  removed: number;
}

export interface AgentRecord {
  id: string;
  name: string;
  slug: string;
  role: string;
  model: string | null;
  system_prompt: string | null;
  mcp_servers: string[];
  max_concurrent_sessions: number;
  is_seeded: boolean;
  active: boolean;
}

/** What revoking an agent would interrupt. */
export interface RevokeImpact {
  live_sessions: number;
  assigned_tasks: string[];
  branch: string | null;
}

/** Which repository the app is working on. */
export interface ProjectInfo {
  path: string | null;
  name: string | null;
}

/** A repository registered in this workspace. */
export interface ProjectRow {
  id: string;
  name: string;
  path: string;
  /** False once the directory has been moved or deleted. */
  exists: boolean;
}

/** What a folder is, before committing to using it. */
export interface FolderInfo {
  path: string;
  name: string;
  is_repository: boolean;
  /** False for a repository with no commits, which cannot host a worktree yet. */
  has_commits: boolean;
}

/** Something the operator told the supervisor to take into account. */
export interface Guidance {
  id: string;
  text: string;
  given_at_iteration: number;
  replan: boolean;
}

/** A run this project has had before. */
export interface PastRunSummary {
  run_id: string;
  objective: string;
  status: string;
  autonomy: string;
  iteration: number;
  spent_usd: number;
  task_count: number;
  decision_count: number;
}
