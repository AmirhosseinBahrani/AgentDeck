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
