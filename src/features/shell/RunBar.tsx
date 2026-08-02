import { Loader2, Moon, Search, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { useTheme } from "../../lib/theme";
import type { RunSnapshot } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The top bar, now that the rail carries identity.
 *
 * It says one thing: what the supervisor is doing and how long it has been doing it. That used
 * to be scattered — the phase in a pill, the elapsed time inside the objective header, the cost
 * in the footer — which meant the answer to "is this still working" was assembled from three
 * places. Here it reads left to right in one line.
 */
export function RunBar({ snapshot }: { snapshot: RunSnapshot | null }) {
  const [now, setNow] = useState(() => Date.now());
  const [theme, setTheme] = useTheme();

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const running = !!snapshot?.active;
  const phase = snapshot?.phase ?? "";
  const blocked = phase === "blockedonhuman" || (snapshot?.open_escalations ?? 0) > 0;
  // Terminal phases are still "active" for a moment while the loop unwinds, and a spinner on a
  // finished run reads as one that will not stop.
  const busy = running && !["completed", "failed", "cancelled", "blockedonhuman"].includes(phase);

  const seconds = snapshot?.started_at_ms
    ? Math.max(0, Math.floor((now - snapshot.started_at_ms) / 1000))
    : 0;

  return (
    <header
      data-tauri-drag-region
      className="flex h-[46px] shrink-0 items-center gap-3 border-b border-deck-line px-4"
    >
      {running ? (
        <>
          <span
            className={cn(
              "size-[7px] shrink-0 rounded-full",
              blocked ? "bg-deck-attention" : "animate-live bg-deck-live",
            )}
          />
          <span className="text-[13px] font-semibold text-deck-text">
            Run {snapshot?.run_id}
          </span>
          <span className="h-4 w-px shrink-0 bg-deck-line" />
          <span className="flex items-center gap-1.5 font-mono text-[11px] text-deck-dim">
            {busy && <Loader2 className="size-3 animate-spin text-deck-accent" />}
            {phaseLabel(phase)}
          </span>
          <span className="font-mono text-[11px] text-deck-faint">iteration {snapshot?.iteration}</span>
          <span className="font-mono text-[11px] tabular-nums text-deck-faint">
            {formatElapsed(seconds)}
          </span>
        </>
      ) : (
        <span className="text-[13px] text-deck-faint">No run</span>
      )}

      <div className="grow" />

      {snapshot?.spent_usd ? (
        <span className="font-mono text-[11px] text-deck-dim">
          ${snapshot.spent_usd.toFixed(2)}
        </span>
      ) : null}

      <button
        onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
        title={theme === "dark" ? "Switch to light" : "Switch to dark"}
        className="flex size-7 items-center justify-center rounded-md text-deck-faint transition-colors hover:bg-deck-raised hover:text-deck-text"
      >
        {theme === "dark" ? <Moon className="size-3.5" /> : <Sun className="size-3.5" />}
      </button>

      <label className="flex h-7 w-[190px] shrink-0 items-center gap-2 rounded-md border border-deck-line bg-deck-surface px-2.5">
        <Search className="size-3 shrink-0 text-deck-faint" />
        <input
          placeholder="Search everything"
          className="min-w-0 grow bg-transparent text-[11.5px] text-deck-text placeholder:text-deck-faint focus:outline-none"
        />
        <kbd className="shrink-0 font-mono text-[10px] text-deck-faint">⌘K</kbd>
      </label>
    </header>
  );
}

function phaseLabel(phase: string): string {
  const labels: Record<string, string> = {
    planning: "planning",
    dispatching: "starting agents",
    monitoring: "working",
    reviewing: "reviewing",
    replanning: "replanning",
    blockedonhuman: "needs you",
  };
  return labels[phase] ?? phase;
}

function formatElapsed(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const mins = Math.floor(seconds / 60);
  if (mins < 60) return `${mins}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${Math.floor(mins / 60)}h ${String(mins % 60).padStart(2, "0")}m`;
}
