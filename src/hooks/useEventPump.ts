import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useSyncExternalStore } from "react";
import { transcriptStore } from "../lib/transcriptStore";
import { ingestEscalations } from "./useEscalations";
import type { EventBatch, EventEnvelope, TranscriptRow } from "../lib/types";

/**
 * Ingests batched events from Rust.
 *
 * Batches land in a mutable buffer and are drained once per animation frame, so a burst of
 * several hundred events causes one flush rather than one React update each. Rust already
 * coalesces on its side; this is the second stage, and the one that bounds render work.
 */

interface PumpStats {
  batches: number;
  events: number;
  gaps: number;
  lastSeq: number;
}

const stats: PumpStats = { batches: 0, events: 0, gaps: 0, lastSeq: 0 };
const statsListeners = new Set<() => void>();

function bumpStats(patch: Partial<PumpStats>) {
  Object.assign(stats, patch);
  for (const l of statsListeners) l();
}

export function usePumpStats(): PumpStats {
  return useSyncExternalStore(
    (cb) => {
      statsListeners.add(cb);
      return () => statsListeners.delete(cb);
    },
    () => stats,
  );
}

export function useEventPump(): void {
  const pending = useRef<EventEnvelope[]>([]);
  const frame = useRef<number | null>(null);
  const started = useRef(false);

  useEffect(() => {
    // Guard against StrictMode's double-invoke opening two channels, which would double
    // every event.
    if (started.current) return;
    started.current = true;

    const flush = () => {
      frame.current = null;
      const batch = pending.current;
      if (batch.length === 0) return;
      pending.current = [];
      transcriptStore.ingest(batch);
      bumpStats({ events: stats.events + batch.length });
    };

    const schedule = () => {
      if (frame.current === null) {
        frame.current = requestAnimationFrame(flush);
      }
    };

    const channel = new Channel<EventBatch>();
    channel.onmessage = (batch) => {
      // A gap means the observer channel dropped events. Backfill rather than silently
      // rendering an incomplete transcript.
      if (batch.first_seq !== null && stats.lastSeq > 0 && batch.first_seq > stats.lastSeq + 1) {
        bumpStats({ gaps: stats.gaps + 1 });
        void backfill(stats.lastSeq).then((missed) => {
          if (missed.length > 0) transcriptStore.ingest(missed);
        });
      }

      for (const delta of batch.deltas) {
        transcriptStore.appendPartial(delta.session_id, delta.text);
      }

      if (batch.events.length > 0) {
        // Escalations are handled synchronously: they arrive unbatched precisely because
        // latency matters, so deferring them to the next frame would waste that.
        ingestEscalations(batch.events);
        pending.current.push(...batch.events);
        schedule();
      }

      bumpStats({
        batches: stats.batches + 1,
        lastSeq: batch.last_seq ?? stats.lastSeq,
      });
    };

    void invoke("subscribe_events", { channel });

    return () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
    };
  }, []);
}

async function backfill(afterSeq: number): Promise<EventEnvelope[]> {
  try {
    return await invoke<EventEnvelope[]>("get_events_since", {
      // String, because seq is a u64 on the Rust side and would lose precision as a JS number.
      after: String(afterSeq),
      limit: 1000,
    });
  } catch {
    return [];
  }
}

/** Subscribes to one session's rows. Sessions with no hook mounted cost nothing. */
export function useTranscript(sessionId: string | null): TranscriptRow[] {
  return useSyncExternalStore(
    (cb) => (sessionId ? transcriptStore.subscribeRows(sessionId, cb) : () => {}),
    () => (sessionId ? transcriptStore.getRows(sessionId) : EMPTY),
  );
}

/**
 * Restores a session's scrollback from the database the first time it is opened.
 *
 * Runs once per session id. The pump is a live feed and holds nothing from before the app
 * started, so without this every transcript from an earlier run was an empty pane.
 */
export function useTranscriptBackfill(sessionId: string | null): void {
  const done = useRef(new Set<string>());

  useEffect(() => {
    if (!sessionId || done.current.has(sessionId)) return;
    done.current.add(sessionId);

    void invoke<EventEnvelope[]>("get_session_transcript", { sessionId, limit: 2000 })
      .then((events) => transcriptStore.hydrate(sessionId, events))
      .catch(() => {
        // Leaves the pane as the live feed has it. Retrying on a failed read would just repeat
        // whatever made it fail.
        done.current.delete(sessionId);
      });
  }, [sessionId]);
}

/** The streaming tail. Separate hook so committed rows do not re-render per token. */
export function usePartial(sessionId: string | null): string {
  return useSyncExternalStore(
    (cb) => (sessionId ? transcriptStore.subscribePartial(sessionId, cb) : () => {}),
    () => (sessionId ? transcriptStore.getPartial(sessionId) : ""),
  );
}

/** Tells Rust which sessions are visible so it can drop deltas for the rest at the source. */
export function useSessionSubscriptions(sessionIds: string[]): void {
  const key = sessionIds.join(",");
  useEffect(() => {
    void invoke("set_session_subscriptions", { sessions: sessionIds }).catch(() => {});
  }, [key]);
}

const EMPTY: TranscriptRow[] = [];
