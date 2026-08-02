import {
  FileText,
  FolderGit2,
  GitCompare,
  type LucideIcon,
  Network,
  Play,
  Shield,
  Sparkles,
  SquareTerminal,
  Users,
} from "lucide-react";
import type { Autonomy, NavTab, RunSnapshot } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The standing navigation, down the left edge.
 *
 * It replaced a row of tabs, and the reason is the shape of the product rather than taste. Ten
 * destinations across the top forces every label to compete on one line, with no room to say
 * what any of them is holding — and a supervised team is exactly the case where "how many agents
 * / how many tasks / how many decisions" is the thing you navigate by. A column gives each
 * destination its own line, a count in the same lane, and room to group.
 *
 * The three groups answer three different questions: what is happening now, what did it produce,
 * and what are the standing rules. Autonomy is pinned to the foot because it is a property of
 * the whole app rather than of any one view, and because the answer to "can agents act without
 * asking me" should never be more than a glance away.
 */
export function AppRail({
  active,
  onChange,
  project,
  projectPath,
  onChangeProject,
  projectCount,
  snapshot,
  sessionCount,
}: {
  active: NavTab;
  onChange: (tab: NavTab) => void;
  project: string;
  projectPath?: string;
  onChangeProject: () => void;
  projectCount: number;
  snapshot: RunSnapshot | null;
  sessionCount: number;
}) {
  const running = !!snapshot?.active;
  const blocked = (snapshot?.open_escalations ?? 0) > 0;

  return (
    <aside className="glass-flat flex w-[236px] shrink-0 flex-col border-r border-deck-line">
      <div data-tauri-drag-region className="flex h-[46px] shrink-0 items-center gap-2.5 pl-20">
        <Mark />
        <span className="text-[14px] font-semibold tracking-[-0.02em] text-deck-text">
          AgentDeck
        </span>
      </div>

      <div className="px-3 pb-3.5">
        <button
          onClick={onChangeProject}
          title={projectPath ?? "Choose a repository for agents to work in"}
          className="flex w-full items-center gap-2 rounded-lg border border-deck-line bg-deck-bg px-2.5 py-2 text-left transition-colors hover:border-deck-line-strong"
        >
          <FolderGit2 className="size-3.5 shrink-0 text-deck-faint" />
          <span className="min-w-0 grow truncate text-[12.5px] font-medium text-deck-text">
            {project}
          </span>
          <span className="shrink-0 font-mono text-[10px] text-deck-faint">{projectCount}</span>
        </button>
      </div>

      <nav className="flex min-h-0 grow flex-col gap-0.5 overflow-y-auto px-3">
        <Group label="Control" />
        <Item
          icon={Play}
          label="Run"
          tab="team"
          active={active}
          onChange={onChange}
          // A dot rather than a number: what matters here is whether anything is happening at
          // all, and a count would compete with the counts every other row is carrying.
          dot={blocked ? "attention" : running ? "live" : undefined}
        />
        <Item
          icon={SquareTerminal}
          label="Sessions"
          tab="workspace"
          active={active}
          onChange={onChange}
          count={sessionCount || undefined}
        />

        <Group label="Inspect" />
        <Item icon={Network} label="Task graph" tab="graph" active={active} onChange={onChange} count={snapshot?.tasks.length} />
        <Item icon={GitCompare} label="Diffs" tab="diffs" active={active} onChange={onChange} />
        <Item icon={FileText} label="Files" tab="files" active={active} onChange={onChange} />
        <Item
          icon={Shield}
          label="Decisions"
          tab="decisions"
          active={active}
          onChange={onChange}
          count={snapshot?.decisions.length}
        />

        <Group label="Configure" />
        <Item icon={SquareTerminal} label="Supervisor" tab="supervisor" active={active} onChange={onChange} />
        <Item icon={FileText} label="Memory" tab="memory" active={active} onChange={onChange} />
        <Item icon={Sparkles} label="Skills" tab="skills" active={active} onChange={onChange} />
        {/* Where the roster's numbers live, alongside models and permissions — one view, so one
            entry. A second row pointing at it would light up in step with this one. */}
        <Item
          icon={Users}
          label="Team & settings"
          tab="advanced"
          active={active}
          onChange={onChange}
          count={snapshot?.agents.length}
        />
      </nav>

      <AutonomyFoot
        mode={snapshot?.autonomy || "assisted"}
        running={running}
        maxConcurrent={snapshot?.max_concurrent ?? 0}
      />
    </aside>
  );
}

/** The hub and its workers — the org model the whole app is built on. */
function Mark() {
  return (
    <svg width="17" height="17" viewBox="0 0 16 16" fill="none" className="shrink-0">
      <g stroke="var(--deck-accent)" strokeWidth="1.15" strokeLinecap="round" fill="none">
        <line x1="8" y1="4.9" x2="8" y2="3.1" />
        <circle cx="8" cy="1.9" r="1.15" />
        <line x1="10.19" y1="5.81" x2="11.46" y2="4.54" />
        <circle cx="12.31" cy="3.69" r="1.15" />
        <line x1="11.1" y1="8" x2="12.9" y2="8" />
        <circle cx="14.1" cy="8" r="1.15" />
        <line x1="10.19" y1="10.19" x2="11.46" y2="11.46" />
        <circle cx="12.31" cy="12.31" r="1.15" />
        <line x1="8" y1="11.1" x2="8" y2="12.9" />
        <circle cx="8" cy="14.1" r="1.15" />
        <line x1="5.81" y1="10.19" x2="4.54" y2="11.46" />
        <circle cx="3.69" cy="12.31" r="1.15" />
        <line x1="4.9" y1="8" x2="3.1" y2="8" />
        <circle cx="1.9" cy="8" r="1.15" />
        <line x1="5.81" y1="5.81" x2="4.54" y2="4.54" />
        <circle cx="3.69" cy="3.69" r="1.15" />
        <circle cx="8" cy="8" r="2.9" />
      </g>
      <circle cx="8" cy="8" r="1.5" fill="var(--deck-accent)" />
    </svg>
  );
}

function Group({ label }: { label: string }) {
  return <span className="label-micro px-2.5 pt-5 pb-2">{label}</span>;
}

function Item({
  icon: Icon,
  label,
  tab,
  active,
  onChange,
  count,
  dot,
}: {
  icon: LucideIcon;
  label: string;
  tab: NavTab;
  active: NavTab;
  onChange: (tab: NavTab) => void;
  count?: number;
  dot?: "live" | "attention";
}) {
  const on = active === tab;
  return (
    <button
      onClick={() => onChange(tab)}
      className={cn(
        "flex h-8 items-center gap-2.5 rounded-[7px] px-2.5 text-left transition-colors",
        on ? "bg-deck-accent-wash" : "hover:bg-deck-raised",
      )}
    >
      <span className="flex w-4 shrink-0 justify-center">
        <Icon className={cn("size-[15px]", on ? "text-deck-accent" : "text-deck-dim")} />
      </span>
      <span
        className={cn(
          "grow truncate text-[13px]",
          on ? "font-semibold text-deck-accent" : "text-deck-text",
        )}
      >
        {label}
      </span>
      {/* A fixed trailing lane, present even when empty, so counts and dots line up down the
          column instead of drifting with each label's width. */}
      <span className="flex w-6 shrink-0 items-center justify-end">
        {dot ? (
          <span
            className={cn(
              "size-1.5 rounded-full",
              dot === "attention" ? "bg-deck-attention" : "animate-live bg-deck-live",
            )}
          />
        ) : count !== undefined && count > 0 ? (
          <span className="font-mono text-[10px] text-deck-faint">{count}</span>
        ) : null}
      </span>
    </button>
  );
}

function AutonomyFoot({
  mode,
  running,
  maxConcurrent,
}: {
  mode: Autonomy;
  running: boolean;
  maxConcurrent: number;
}) {
  const label = mode?.trim() ? mode : "assisted";
  const copy: Record<string, string> = {
    manual: "You start each agent yourself. Nothing is retried without you.",
    assisted: "Agents start on their own. You are asked only when something needs deciding.",
    autonomous: "Runs unattended. You are told what happened rather than asked first.",
  };

  return (
    <div className="flex shrink-0 flex-col gap-2 border-t border-deck-line px-4 py-4">
      <div className="flex items-baseline justify-between">
        <span className="label-micro">Autonomy</span>
        {maxConcurrent > 0 && (
          <span className="font-mono text-[10px] text-deck-faint">{maxConcurrent} max</span>
        )}
      </div>

      <div className="flex rounded-lg border border-deck-line bg-deck-bg p-0.5">
        {(["manual", "assisted", "autonomous"] as const).map((option) => (
          <span
            key={option}
            className={cn(
              "flex h-6 flex-1 items-center justify-center rounded-md text-[11px] capitalize",
              option === label
                ? // The current mode, not a button: autonomy is chosen where a run is started,
                  // and a control here would imply it could change mid-run.
                  "bg-deck-text font-semibold text-deck-bg"
                : "font-medium text-deck-faint",
            )}
          >
            {option === "autonomous" ? "auto" : option}
          </span>
        ))}
      </div>

      <p className="text-[11px] leading-4 text-deck-dim">{copy[label] ?? copy.assisted}</p>
      {running && label === "autonomous" && (
        <span className="font-mono text-[10px] text-deck-attention">unattended</span>
      )}
    </div>
  );
}
