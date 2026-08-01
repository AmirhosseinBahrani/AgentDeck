import type { AgentEvent, EventEnvelope, TranscriptRow } from "./types";

/**
 * Transcript storage, deliberately outside React and outside Zustand.
 *
 * This is the boundary that makes many concurrent agents affordable. Rows are appended to a
 * per-session ring buffer; a session with no subscriber has no listeners to notify, so
 * appending to a background agent's transcript does literally zero React work. Putting this
 * in a global store instead would re-render on every event from every agent.
 *
 * Token deltas never become rows. They accumulate in a separate `partial` slot with its own
 * listeners, so during generation only the streaming tail re-renders — not the list.
 */

const MAX_ROWS = 2000;

type Listener = () => void;

interface SessionState {
  rows: TranscriptRow[];
  partial: string;
  rowListeners: Set<Listener>;
  partialListeners: Set<Listener>;
  /** Bumped on every mutation so useSyncExternalStore can return a stable snapshot. */
  version: number;
  droppedFromHead: number;
}

function emptySession(): SessionState {
  return {
    rows: [],
    partial: "",
    rowListeners: new Set(),
    partialListeners: new Set(),
    version: 0,
    droppedFromHead: 0,
  };
}

function stringifyOutput(output: unknown): string {
  if (typeof output === "string") return output;
  if (output == null) return "";
  // Tool results arrive as either a string or content blocks; flatten blocks to their text.
  if (Array.isArray(output)) {
    return output
      .map((block) =>
        block && typeof block === "object" && "text" in block
          ? String((block as { text: unknown }).text)
          : JSON.stringify(block),
      )
      .join("\n");
  }
  return JSON.stringify(output);
}

/** Events that carry no transcript-visible content. Dropping them here keeps the row list
 *  meaningful rather than padded with invisible entries that break virtualization sizing. */
function toRow(envelope: EventEnvelope): TranscriptRow | null {
  const e: AgentEvent = envelope.event;
  const id = String(envelope.seq);

  switch (e.kind) {
    case "message": {
      const content = e.content as { type?: string; text?: string } | null;
      if (!content || content.type !== "text" || !content.text) return null;
      return { id, type: "text", role: e.role, text: content.text };
    }
    case "tool_call":
      return { id, type: "tool_call", tool: e.tool, input: e.input };
    case "tool_result":
      return {
        id,
        type: "tool_result",
        toolUseId: e.tool_use_id,
        output: stringifyOutput(e.output),
        isError: e.is_error,
      };
    case "permission_request":
      return {
        id,
        type: "permission",
        requestId: e.request_id,
        tool: e.tool,
        reason: e.reason_type,
      };
    case "turn_complete":
      return { id, type: "turn", costUsd: e.cost_usd, isError: e.is_error };
    case "session_ready":
      return {
        id,
        type: "notice",
        text: `Session started in ${e.cwd}${e.model ? ` on ${e.model}` : ""}`,
        severity: "info",
      };
    case "session_exited":
      return {
        id,
        type: "notice",
        text: `Session ended (${typeof e.reason === "string" ? e.reason : Object.keys(e.reason)[0]})`,
        severity: typeof e.reason === "string" && e.reason === "clean" ? "info" : "warn",
      };
    case "rate_limited":
      return {
        id,
        type: "notice",
        text: e.resets_at
          ? `Rate limited (${e.limit_type ?? "unknown"}), resets ${new Date(e.resets_at * 1000).toLocaleTimeString()}`
          : `Rate limit status: ${e.status}`,
        severity: e.status === "allowed" ? "info" : "warn",
      };
    case "diagnostic":
      return { id, type: "notice", text: e.message, severity: "error" };
    // Turn boundaries, resolved permissions and unmodelled events are state, not content.
    default:
      return null;
  }
}

class TranscriptStore {
  private sessions = new Map<string, SessionState>();

  private session(id: string): SessionState {
    let s = this.sessions.get(id);
    if (!s) {
      s = emptySession();
      this.sessions.set(id, s);
    }
    return s;
  }

  /** Appends events. Only sessions with subscribers cause any notification. */
  ingest(envelopes: EventEnvelope[]): void {
    const touched = new Set<string>();

    for (const envelope of envelopes) {
      const sessionId = envelope.session_id;
      if (!sessionId) continue;
      const row = toRow(envelope);
      if (!row) continue;

      const s = this.session(sessionId);
      // A completed message supersedes whatever partial text was streaming into it.
      if (row.type === "text") s.partial = "";
      s.rows.push(row);
      if (s.rows.length > MAX_ROWS) {
        s.rows.shift();
        s.droppedFromHead += 1;
      }
      s.version += 1;
      touched.add(sessionId);
    }

    for (const id of touched) this.notifyRows(id);
  }

  /**
   * Fills a session's scrollback from the durable event log.
   *
   * The live pump only carries what has happened since the app started, so a session from an
   * earlier run — or one whose events arrived before its tab was opened — had no rows at all and
   * rendered as a blank pane. Merged by seq rather than appended: a live event can land between
   * the caller reading the transcript and this running, and the same row twice is worse than the
   * gap it would be papering over.
   */
  hydrate(sessionId: string, envelopes: EventEnvelope[]): void {
    const s = this.session(sessionId);
    const known = new Set(s.rows.map((r) => r.id));

    const restored: TranscriptRow[] = [];
    for (const envelope of envelopes) {
      const row = toRow(envelope);
      if (row && !known.has(row.id)) restored.push(row);
    }
    if (restored.length === 0) return;

    s.rows = [...restored, ...s.rows]
      .sort((a, b) => Number(a.id) - Number(b.id))
      .slice(-MAX_ROWS);
    s.version += 1;
    this.notifyRows(sessionId);
  }

  /** Buffers streaming text. Notifies only the tail listeners, never the row list. */
  appendPartial(sessionId: string, text: string): void {
    const s = this.session(sessionId);
    s.partial += text;
    if (s.partialListeners.size > 0) {
      for (const l of s.partialListeners) l();
    }
  }

  private notifyRows(id: string): void {
    const s = this.sessions.get(id);
    if (!s) return;
    for (const l of s.rowListeners) l();
  }

  subscribeRows(sessionId: string, listener: Listener): () => void {
    const s = this.session(sessionId);
    s.rowListeners.add(listener);
    return () => {
      s.rowListeners.delete(listener);
    };
  }

  subscribePartial(sessionId: string, listener: Listener): () => void {
    const s = this.session(sessionId);
    s.partialListeners.add(listener);
    return () => {
      s.partialListeners.delete(listener);
    };
  }

  getRows(sessionId: string): TranscriptRow[] {
    return this.sessions.get(sessionId)?.rows ?? EMPTY_ROWS;
  }

  getPartial(sessionId: string): string {
    return this.sessions.get(sessionId)?.partial ?? "";
  }

  /** Identity changes only when rows change, so useSyncExternalStore does not loop. */
  getRowsVersion(sessionId: string): number {
    return this.sessions.get(sessionId)?.version ?? 0;
  }

  hasSubscribers(sessionId: string): boolean {
    const s = this.sessions.get(sessionId);
    return !!s && (s.rowListeners.size > 0 || s.partialListeners.size > 0);
  }

  knownSessions(): string[] {
    return [...this.sessions.keys()];
  }

  reset(): void {
    this.sessions.clear();
  }
}

const EMPTY_ROWS: TranscriptRow[] = [];

export const transcriptStore = new TranscriptStore();
export { toRow, MAX_ROWS };
