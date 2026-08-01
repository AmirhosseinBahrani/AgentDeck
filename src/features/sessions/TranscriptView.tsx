import { useVirtualizer } from "@tanstack/react-virtual";
import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { usePartial, useTranscript, useTranscriptBackfill } from "../../hooks/useEventPump";
import type { TranscriptRow } from "../../lib/types";

/**
 * Virtualized transcript.
 *
 * Two things keep this cheap with several agents streaming: rows are memoized with
 * identity-stable props so an append re-renders one row rather than the list, and streaming
 * text lives in a sibling component that subscribes separately — during generation only the
 * tail repaints.
 */
export function TranscriptView({ sessionId }: { sessionId: string | null }) {
  useTranscriptBackfill(sessionId);
  const rows = useTranscript(sessionId);
  const parentRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 56,
    overscan: 8,
    getItemKey: (i) => rows[i].id,
  });

  // Only follow the tail when the user is already at the bottom. Yanking them back while
  // they are reading scrollback is the classic log-viewer annoyance.
  useLayoutEffect(() => {
    if (pinned && rows.length > 0) {
      virtualizer.scrollToIndex(rows.length - 1, { align: "end" });
    }
  }, [rows.length, pinned, virtualizer]);

  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const onScroll = () => {
      const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
      setPinned(distance < 40);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  if (!sessionId) {
    return (
      <div className="flex h-full items-center justify-center text-deck-faint">
        Select or start a session
      </div>
    );
  }

  return (
    <div className="relative flex h-full flex-col">
      <div ref={parentRef} className="flex-1 overflow-y-auto px-4 py-2">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => (
            <div
              key={item.key}
              ref={virtualizer.measureElement}
              data-index={item.index}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                transform: `translateY(${item.start}px)`,
              }}
            >
              <Row row={rows[item.index]} />
            </div>
          ))}
        </div>
        <StreamingTail sessionId={sessionId} />
      </div>

      {!pinned && (
        <button
          onClick={() => setPinned(true)}
          className="absolute right-4 bottom-4 rounded-md border border-white/[0.1] bg-deck-raised px-3 py-1.5 text-xs text-deck-text shadow-lg transition-colors hover:bg-white/[0.14]"
        >
          Jump to latest
        </button>
      )}
    </div>
  );
}

/** The only component that re-renders per token. */
function StreamingTail({ sessionId }: { sessionId: string }) {
  const partial = usePartial(sessionId);
  if (!partial) return null;
  return (
    <div className="py-1.5 font-mono text-[12px] whitespace-pre-wrap text-deck-text">
      {partial}
      <span className="animate-live ml-0.5 inline-block h-3.5 w-1.5 bg-deck-live align-middle" />
    </div>
  );
}

const Row = memo(function Row({ row }: { row: TranscriptRow }) {
  switch (row.type) {
    case "text":
      return (
        <div className="py-1.5 text-[13px] leading-relaxed whitespace-pre-wrap text-deck-text">
          {row.text}
        </div>
      );

    case "tool_call":
      return (
        <div className="my-1 rounded-md border border-white/[0.07] bg-white/[0.04] px-2.5 py-1.5">
          <div className="mb-0.5 font-mono text-[11px] tracking-wide text-deck-live">{row.tool}</div>
          <pre className="overflow-x-auto font-mono text-[11px] text-deck-dim">
            {truncate(JSON.stringify(row.input, null, 2), 600)}
          </pre>
        </div>
      );

    case "tool_result":
      return (
        <div
          className={`my-1 rounded border px-2.5 py-1.5 ${
            row.isError
              ? "border-deck-danger/35 bg-deck-danger/10"
              : "border-white/[0.06] bg-white/[0.025]"
          }`}
        >
          <pre
            className={`overflow-x-auto font-mono text-[11px] whitespace-pre-wrap ${
              row.isError ? "text-deck-danger" : "text-deck-dim"
            }`}
          >
            {truncate(row.output, 1200)}
          </pre>
        </div>
      );

    case "permission":
      return (
        <div className="my-1 rounded border border-deck-attention/30 bg-deck-attention/8 px-2.5 py-2">
          <div className="text-[12px] font-medium text-deck-attention">
            Permission required: {row.tool}
          </div>
          {row.reason && (
            <div className="mt-0.5 text-[11px] text-deck-attention/70">Reason: {row.reason}</div>
          )}
        </div>
      );

    case "turn":
      return (
        <div className="flex items-center gap-2 py-1 text-[11px] text-deck-faint">
          <div className="h-px flex-1 bg-white/8" />
          <span>
            turn complete
            {row.costUsd !== null && ` · $${row.costUsd.toFixed(4)}`}
            {row.isError && " · error"}
          </span>
          <div className="h-px flex-1 bg-white/8" />
        </div>
      );

    case "notice":
      return (
        <div
          className={`py-1 text-[11px] ${
            row.severity === "error"
              ? "text-deck-danger"
              : row.severity === "warn"
                ? "text-deck-attention"
                : "text-deck-faint"
          }`}
        >
          {row.text}
        </div>
      );
  }
});

function truncate(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, max)}\n… ${text.length - max} more chars`;
}
