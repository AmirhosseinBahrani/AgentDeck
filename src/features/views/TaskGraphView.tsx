import { SectionRule } from "../../components/ui/section-rule";
import type { RunSnapshot } from "../../lib/types";
import { TaskGraph } from "../team/TaskGraph";

/**
 * The plan, given the whole screen.
 *
 * The same graph the Team view shows in a strip, without the competition for space — which
 * matters once a plan is more than a handful of tasks, because the question this answers is
 * "what is blocking what", and that is exactly the thing a cramped horizontal scroll hides.
 */
export function TaskGraphView({
  snapshot,
  onOpenSession,
}: {
  snapshot: RunSnapshot | null;
  onOpenSession: (sessionId: string) => void;
}) {
  const tasks = snapshot?.tasks ?? [];
  const gates = tasks.filter((t) => t.objective_gate).length;

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 overflow-auto px-7 py-5">
      <SectionRule
        label="Task graph"
        trailing={
          tasks.length > 0
            ? `${tasks.length} task${tasks.length === 1 ? "" : "s"} · ${gates} gating the objective`
            : undefined
        }
      />
      <TaskGraph
        tasks={tasks}
        edges={snapshot?.edges ?? []}
        onOpenSession={onOpenSession}
      />
      {tasks.length > 0 && (
        <p className="pt-2 text-[11px] leading-relaxed text-deck-faint">
          Columns are dependency depth — everything in a column can run at once, and an edge
          always points rightward. A task marked <span className="text-deck-dim">gate</span>{" "}
          must finish before the objective can be called done.
        </p>
      )}
    </div>
  );
}
