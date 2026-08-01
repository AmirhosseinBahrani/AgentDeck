import type { RunSnapshot } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The standing context: which repository, who is on the team, and how the work is distributed.
 *
 * Always visible, because these are the things you orient by rather than look up. The counts are
 * derived from the same task list the rest of the screen renders, so the sidebar cannot disagree
 * with the panel next to it — which it would if it queried separately.
 */
export function WorkspaceSidebar({
  snapshot,
  activeSession,
  onOpenSession,
}: {
  snapshot: RunSnapshot | null;
  activeSession: string | null;
  onOpenSession: (sessionId: string) => void;
}) {
  const tasks = snapshot?.tasks ?? [];
  const counts = [
    { label: "Running", n: tasks.filter((t) => t.status === "running").length, tone: "live" },
    { label: "In review", n: tasks.filter((t) => t.status === "review").length, tone: "live" },
    {
      label: "Blocked",
      n: tasks.filter((t) => t.status === "blocked").length,
      tone: "attention",
    },
    {
      label: "Queued",
      n: tasks.filter((t) => ["queued", "assigned", "backlog"].includes(t.status)).length,
      tone: "dim",
    },
    {
      label: "Completed",
      n: tasks.filter((t) => t.status === "completed").length,
      tone: "faint",
    },
  ] as const;

  return (
    <aside className="glass-flat flex w-[236px] shrink-0 flex-col gap-[22px] overflow-y-auto border-r border-white/[0.07] px-3 py-[18px]">
      <Group label="Workspace">
        <div className="flex h-7 items-center gap-2 rounded-md bg-white/[0.05] px-2">
          <span className="grow truncate text-[12.5px] font-semibold text-deck-text">
            {projectName(snapshot)}
          </span>
          <span className="font-mono text-[10px] text-deck-faint">{tasks.length}</span>
        </div>
      </Group>

      <Group label="Team" trailing={String(snapshot?.agents.length ?? 0)}>
        {(snapshot?.agents ?? []).map((agent) => {
          const active = !!agent.session_id && agent.session_id === activeSession;
          return (
            <button
              key={agent.id}
              disabled={!agent.session_id}
              onClick={() => agent.session_id && onOpenSession(agent.session_id)}
              className={cn(
                "flex h-[31px] items-center gap-2 rounded-md px-2 text-left transition-colors",
                active && "bg-deck-live/[0.09] ring-1 ring-deck-live/25",
                !active && agent.session_id && "hover:bg-white/[0.05]",
                agent.status === "blocked" && !active && "bg-deck-attention/[0.08]",
              )}
            >
              <span className="flex w-2.5 shrink-0 justify-center">
                <span
                  className={cn(
                    "size-1.5 rounded-full",
                    agent.status === "running" && "animate-live bg-deck-live",
                    agent.status === "blocked" && "bg-deck-attention",
                    agent.status === "idle" && "border border-deck-faint/70",
                  )}
                />
              </span>
              <span
                className={cn(
                  "min-w-0 grow truncate text-[12.5px] font-medium",
                  agent.status === "idle" ? "text-deck-faint" : "text-deck-text",
                )}
              >
                {agent.name}
              </span>
              <span
                className={cn(
                  "shrink-0 font-mono text-[10.5px]",
                  agent.status === "running" && "text-deck-live",
                  agent.status === "blocked" && "text-deck-attention",
                  agent.status === "idle" && "text-deck-faint",
                )}
              >
                {agent.status}
              </span>
            </button>
          );
        })}
        {(snapshot?.agents.length ?? 0) === 0 && (
          <p className="px-2 text-[11px] leading-relaxed text-deck-faint">
            The team appears once a run starts.
          </p>
        )}
      </Group>

      <Group label="Tasks" trailing={String(tasks.length)}>
        {counts.map((c) => (
          <div key={c.label} className="flex h-7 items-center gap-2 rounded-md px-2">
            <span className="flex w-2.5 shrink-0 justify-center">
              <span
                className={cn(
                  "size-1.5 rounded-full",
                  c.n === 0
                    ? "border border-deck-faint/40"
                    : c.tone === "live"
                      ? "bg-deck-live"
                      : c.tone === "attention"
                        ? "bg-deck-attention"
                        : c.tone === "faint"
                          ? "bg-deck-done"
                          : "bg-deck-dim",
                )}
              />
            </span>
            <span
              className={cn(
                "grow text-[12px]",
                c.n === 0 ? "text-deck-faint" : "text-deck-dim",
              )}
            >
              {c.label}
            </span>
            <span
              className={cn(
                "shrink-0 font-mono text-[10.5px]",
                c.n === 0 ? "text-deck-faint" : "text-deck-dim",
              )}
            >
              {c.n}
            </span>
          </div>
        ))}
      </Group>
    </aside>
  );
}

function Group({
  label,
  trailing,
  children,
}: {
  label: string;
  trailing?: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-0.5">
      <div className="flex items-center justify-between px-1.5 pb-2">
        <span className="label-micro">{label}</span>
        {trailing && <span className="font-mono text-[10px] text-deck-faint">{trailing}</span>}
      </div>
      {children}
    </section>
  );
}

/** The objective, shortened to something that fits a sidebar. */
function projectName(snapshot: RunSnapshot | null): string {
  const objective = snapshot?.objective?.trim();
  if (!objective) return "No run";
  const words = objective.split(/\s+/).slice(0, 4).join(" ");
  return words.length < objective.length ? `${words}…` : words;
}
