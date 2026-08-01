import type { RunSnapshot, SessionHistoryEntry } from "../../lib/types";
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
  history,
  activeSession,
  activeTask,
  onOpenSession,
  onOpenTask,
}: {
  snapshot: RunSnapshot | null;
  history: SessionHistoryEntry[];
  activeSession: string | null;
  activeTask: string | null;
  onOpenSession: (sessionId: string) => void;
  onOpenTask: (taskId: string) => void;
}) {
  const tasks = snapshot?.tasks ?? [];
  // Sessions belonging to the live run are already listed under Team with their current status.
  // Repeating them here would make the history read as though the run had happened twice.
  const liveSessions = new Set(
    (snapshot?.agents ?? []).map((a) => a.session_id).filter(Boolean) as string[],
  );
  const past = history.filter((h) => !liveSessions.has(h.session_id));

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
                    agent.status === "reviewing" && "bg-deck-live/60",
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
                  (agent.status === "running" || agent.status === "reviewing") &&
                    "text-deck-live",
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
        {tasks.length === 0 && (
          <p className="px-2 text-[11px] leading-relaxed text-deck-faint">
            Tasks appear once the objective is decomposed.
          </p>
        )}
        {tasks.map((task) => {
          const selected = task.id === activeTask;
          return (
            <button
              key={task.id}
              onClick={() => onOpenTask(task.id)}
              title={task.title}
              className={cn(
                "flex h-7 items-center gap-2 rounded-md px-2 text-left transition-colors",
                selected ? "bg-white/[0.07]" : "hover:bg-white/[0.05]",
              )}
            >
              <span className="flex w-2.5 shrink-0 justify-center">
                <span className={cn("size-1.5 rounded-full", taskTone(task.status))} />
              </span>
              <span
                className={cn(
                  "min-w-0 grow truncate text-[12px]",
                  task.status === "completed" ? "text-deck-faint" : "text-deck-dim",
                )}
              >
                {task.title}
              </span>
            </button>
          );
        })}
      </Group>

      <Group label="History" trailing={past.length ? String(past.length) : undefined}>
        {past.length === 0 ? (
          <p className="px-2 text-[11px] leading-relaxed text-deck-faint">
            Sessions from earlier runs are listed here once one has finished.
          </p>
        ) : (
          past.map((entry) => {
            const active = entry.session_id === activeSession;
            return (
              <button
                key={entry.session_id}
                onClick={() => onOpenSession(entry.session_id)}
                title={entry.task_title ?? entry.agent_name}
                className={cn(
                  "flex flex-col gap-px rounded-md px-2 py-1 text-left transition-colors",
                  active ? "bg-white/[0.07]" : "hover:bg-white/[0.05]",
                )}
              >
                <span className="flex items-center gap-2">
                  <span className="min-w-0 grow truncate text-[12px] text-deck-dim">
                    {entry.agent_name}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-deck-faint">
                    {ago(entry.ended_at ?? entry.started_at)}
                  </span>
                </span>
                <span className="truncate text-[11px] text-deck-faint">
                  {entry.task_title ?? entry.status}
                </span>
              </button>
            );
          })
        )}
      </Group>
    </aside>
  );
}

/** Status colour for a task dot, matching the roster's vocabulary. */
function taskTone(status: string): string {
  if (status === "running" || status === "review") return "bg-deck-live";
  if (status === "blocked") return "bg-deck-attention";
  if (status === "completed") return "bg-deck-done";
  if (status === "failed" || status === "cancelled") return "bg-deck-faint";
  return "border border-deck-faint/50";
}

/** Coarse relative time. Precision past the hour is noise in a list you scan. */
function ago(at: number | null): string {
  if (!at) return "";
  const mins = Math.max(0, Math.round((Date.now() - at) / 60_000));
  if (mins < 1) return "now";
  if (mins < 60) return `${mins}m`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.round(hours / 24)}d`;
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
