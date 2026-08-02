import { FolderGit2, Loader2, Moon, Search, Sun } from "lucide-react";
import { useEffect, useState } from "react";
import { useTheme } from "../../lib/theme";
import type { Autonomy } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The window chrome: identity, where you are, and how much the team may do unattended.
 *
 * The autonomy pill sits here rather than in the view because it is a property of the whole
 * app, not of whichever screen is open — and because the answer to "can agents act right now
 * without asking me" should never be more than one glance away.
 */
export function TitleBar({
  project,
  projectPath,
  onChangeProject,
  autonomy,
  running,
  phase,
  startedAt,
}: {
  project: string;
  projectPath?: string;
  onChangeProject: () => void;
  autonomy: Autonomy;
  running: boolean;
  phase: string;
  startedAt: number;
}) {
  return (
    <header
      data-tauri-drag-region
      className="glass-flat flex h-[46px] shrink-0 items-center gap-4 border-b border-deck-line px-4"
    >
      {/* Inset traffic lights: the window is frameless so the content can reach the top edge,
          which means the buttons have to be reserved space rather than drawn by the OS. */}
      <div className="flex w-16 shrink-0 items-center gap-2" />

      <div className="flex items-center gap-2.5">
        {/* The hub and its eight workers: the org model the whole app is built on, which is
            also what the app icon shows. Drawn in the accent rather than the live teal — a mark
            that is always on screen must not read as a run that is always in progress. */}
        <svg width="16" height="16" viewBox="0 0 16 16" className="shrink-0">
          <g
            stroke="var(--color-deck-accent)"
            strokeWidth="1.15"
            strokeLinecap="round"
            fill="none"
          >
          <line x1="8.00" y1="4.90" x2="8.00" y2="3.10" />
          <circle cx="8.00" cy="1.90" r="1.15" />
          <line x1="10.19" y1="5.81" x2="11.46" y2="4.54" />
          <circle cx="12.31" cy="3.69" r="1.15" />
          <line x1="11.10" y1="8.00" x2="12.90" y2="8.00" />
          <circle cx="14.10" cy="8.00" r="1.15" />
          <line x1="10.19" y1="10.19" x2="11.46" y2="11.46" />
          <circle cx="12.31" cy="12.31" r="1.15" />
          <line x1="8.00" y1="11.10" x2="8.00" y2="12.90" />
          <circle cx="8.00" cy="14.10" r="1.15" />
          <line x1="5.81" y1="10.19" x2="4.54" y2="11.46" />
          <circle cx="3.69" cy="12.31" r="1.15" />
          <line x1="4.90" y1="8.00" x2="3.10" y2="8.00" />
          <circle cx="1.90" cy="8.00" r="1.15" />
          <line x1="5.81" y1="5.81" x2="4.54" y2="4.54" />
          <circle cx="3.69" cy="3.69" r="1.15" />
            <circle cx="8" cy="8" r="2.9" />
          </g>
          <circle cx="8" cy="8" r="1.5" fill="var(--color-deck-accent)" />
        </svg>
        <span className="text-[13px] font-semibold tracking-[-0.01em] text-deck-text">
          AgentDeck
        </span>
      </div>

      <div className="h-4 w-px shrink-0 bg-deck-line-strong" />

      {/* The project, not the objective. This is the one piece of context that is true between
          runs, and it doubles as the way to change it — there is nowhere else that would be. */}
      <button
        onClick={onChangeProject}
        title={projectPath ?? "Choose a repository for agents to work in"}
        className="flex min-w-0 items-center gap-1.5 rounded px-1.5 py-0.5 transition-colors hover:bg-deck-raised"
      >
        <FolderGit2 className="size-3 shrink-0 text-deck-faint" />
        <span className="truncate font-mono text-[11px] text-deck-dim">{project}</span>
      </button>

      <div className="grow" />

      {running && <RunStatus phase={phase} startedAt={startedAt} />}

      <AutonomyPill mode={autonomy} running={running} />

      <ThemeToggle />

      <label className="flex h-[27px] w-[200px] shrink-0 items-center gap-2 rounded-md border border-deck-line bg-deck-bg px-2.5">
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

/**
 * Light or dark, as a preference rather than a control.
 *
 * Ghost-quiet and unlabelled: it sits in the same bar as the run status and the autonomy mode,
 * and anything with a fill or an accent here would compete with the two things that actually
 * report on the work. The icon names the theme you are in, which is what someone checks when
 * they glance at it — the destination lives in the tooltip.
 */
function ThemeToggle() {
  const [theme, setTheme] = useTheme();
  const dark = theme === "dark";

  return (
    <button
      onClick={() => setTheme(dark ? "light" : "dark")}
      title={dark ? "Dark theme — switch to light" : "Light theme — switch to dark"}
      className="flex size-[27px] shrink-0 items-center justify-center rounded-md text-deck-faint transition-colors hover:bg-deck-raised hover:text-deck-dim"
    >
      {dark ? <Moon className="size-3.5" /> : <Sun className="size-3.5" />}
    </button>
  );
}

/**
 * What the supervisor is doing right now, and for how long.
 *
 * Planning is a single model call that can take the better part of a minute, during which the
 * roster is idle and the task list is empty — a truthful picture of the state that reads exactly
 * like a run that failed to start. The elapsed counter is the part that does the work: a spinner
 * alone cannot distinguish "thinking" from "wedged", and knowing which is what stops someone
 * pressing Stop on a run that was about to produce something.
 */
function RunStatus({ phase, startedAt }: { phase: string; startedAt: number }) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const seconds = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;
  const busy = !["completed", "failed", "cancelled", "blockedonhuman"].includes(phase);

  return (
    <span
      title="What the supervisor is doing, and how long the run has been going"
      className="flex h-[26px] items-center gap-2 rounded-full border border-deck-line bg-deck-bg px-[11px]"
    >
      {busy && <Loader2 className="size-3 shrink-0 animate-spin text-deck-live" />}
      <span className="font-mono text-[10.5px] tracking-[0.04em] text-deck-dim">
        {phaseLabel(phase)}
      </span>
      <span className="font-mono text-[10.5px] tabular-nums text-deck-faint">
        {formatElapsed(seconds)}
      </span>
    </span>
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

function AutonomyPill({ mode, running }: { mode: Autonomy; running: boolean }) {
  // Guarded rather than trusted. The label is the whole point of the pill, and a value that
  // arrives empty renders a coloured chip that says nothing — which is worse than wrong,
  // because a chip with no word still looks like a deliberate state.
  const label = mode?.trim() ? mode : "assisted";
  const attention = label === "autonomous";
  return (
    <span
      title={`${label} mode — how much the team may do without asking you`}
      className={cn(
        "flex h-[26px] items-center gap-[7px] rounded-full border px-[11px]",
        attention
          ? "border-deck-attention/30 bg-deck-attention/12"
          : "border-deck-live/30 bg-deck-live/12",
      )}
    >
      <span
        className={cn(
          "size-[5px] rounded-full",
          attention ? "bg-deck-attention" : "bg-deck-live",
          running && "animate-live",
        )}
      />
      <span
        className={cn(
          "font-mono text-[10.5px] font-medium tracking-[0.06em] uppercase",
          attention ? "text-deck-attention" : "text-deck-live",
        )}
      >
        {label}
      </span>
    </span>
  );
}
