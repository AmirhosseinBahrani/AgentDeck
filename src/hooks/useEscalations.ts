import { useCallback, useSyncExternalStore } from "react";
import type { EventEnvelope } from "../lib/types";

/**
 * Open permission prompts.
 *
 * Kept in a small external store rather than component state because escalations arrive from
 * the event pump, which is mounted once at the shell — and the prompt must survive navigating
 * between views. An agent stays blocked until it is answered, so losing one on unmount would
 * strand it.
 */

export interface Escalation {
  requestId: string;
  tool: string;
  input: unknown;
  reasonType: string | null;
  blockedPath: string | null;
  suggestions: unknown[];
  sessionId: string | null;
  /** Absolute deadline so the dialog can show a countdown rather than a spinner. */
  expiresAtMs: number | null;
}

/** Matches the broker's `ASK_TIMEOUT`. Only used for display; the backend owns the real clock. */
const ASK_TIMEOUT_MS = 600_000;

let open: Escalation[] = [];
const listeners = new Set<() => void>();

function notify() {
  for (const l of listeners) l();
}

export function ingestEscalations(envelopes: EventEnvelope[]): void {
  let changed = false;

  for (const envelope of envelopes) {
    const e = envelope.event;

    if (e.kind === "permission_request") {
      // Deduplicate: a gap-backfill can replay an event the UI already has.
      if (open.some((x) => x.requestId === e.request_id)) continue;
      open = [
        ...open,
        {
          requestId: e.request_id,
          tool: e.tool,
          input: e.input,
          reasonType: e.reason_type,
          blockedPath: e.blocked_path,
          suggestions: e.suggestions,
          sessionId: envelope.session_id,
          expiresAtMs: envelope.at_ms + ASK_TIMEOUT_MS,
        },
      ];
      changed = true;
    }

    // The backend may resolve a request without the operator — a policy decision, or the
    // timeout firing — so the prompt has to clear on the event, not only on a click.
    if (e.kind === "permission_resolved") {
      const before = open.length;
      open = open.filter((x) => x.requestId !== e.request_id);
      if (open.length !== before) changed = true;
    }

    // A dead session cannot be waiting on anything.
    if (e.kind === "session_exited" && envelope.session_id) {
      const before = open.length;
      open = open.filter((x) => x.sessionId !== envelope.session_id);
      if (open.length !== before) changed = true;
    }
  }

  if (changed) notify();
}

function clear(requestId: string) {
  const before = open.length;
  open = open.filter((x) => x.requestId !== requestId);
  if (open.length !== before) notify();
}

export function useEscalations() {
  const list = useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => open,
  );

  const resolve = useCallback((requestId: string, _allow: boolean) => {
    clear(requestId);
  }, []);

  return { open: list, resolve };
}

/** Test/reset seam. */
export function resetEscalations() {
  open = [];
  notify();
}
