import type { GraphEdge, TaskSummary } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The plan, drawn as the graph it already is.
 *
 * A flat list cannot answer the question an operator actually has when a run stalls — *what is
 * this waiting on* — because dependency is the whole reason anything is queued. Laid out in
 * dependency depth columns, so an edge always points rightward and the critical path reads
 * left to right without any routing cleverness.
 */
export function TaskGraph({
  tasks,
  edges,
  starting,
  onOpenSession,
}: {
  tasks: TaskSummary[];
  edges: GraphEdge[];
  /** Tasks the operator has just approved, before the supervisor has acted on it. */
  starting?: Set<string>;
  onOpenSession: (sessionId: string) => void;
}) {
  if (tasks.length === 0) {
    return (
      <p className="px-1 text-[11px] leading-relaxed text-deck-faint">
        The graph appears once the supervisor has decomposed the objective.
      </p>
    );
  }

  const columns = depthColumns(tasks, edges);

  return (
    <div className="flex items-start gap-3 overflow-x-auto pb-1">
      {columns.map((column, i) => (
        <div key={i} className="flex shrink-0 flex-col gap-2">
          {column.map((task) => (
            <GraphNode
              key={task.id}
              task={task}
              starting={starting?.has(task.id) ?? false}
              onOpenSession={onOpenSession}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

function GraphNode({
  task,
  starting,
  onOpenSession,
}: {
  task: TaskSummary;
  starting: boolean;
  onOpenSession: (sessionId: string) => void;
}) {
  const done = task.status === "completed";
  const running = task.status === "running" || task.status === "review";
  // Starting outranks blocked. An approved task is still flagged awaiting_approval until the
  // supervisor's next iteration, and leaving it amber would show the operator's click having no
  // effect on the one node it was aimed at.
  const blocked = !starting && (task.status === "blocked" || task.awaiting_approval);

  return (
    <button
      onClick={() => task.session_id && onOpenSession(task.session_id)}
      disabled={!task.session_id}
      className={cn(
        "flex w-[152px] flex-col gap-1 rounded-lg border px-2.5 py-2 text-left transition-colors",
        starting && "ring-live animate-live border-deck-live/60 bg-deck-live/[0.14]",
        running && !starting && "border-deck-live/40 bg-deck-live/[0.09]",
        blocked && "border-deck-attention/40 bg-deck-attention/[0.09]",
        done && "border-white/[0.07] bg-white/[0.03]",
        !starting && !running && !blocked && !done && "border-white/[0.06] bg-white/[0.015]",
        task.session_id ? "cursor-pointer hover:brightness-125" : "cursor-default",
      )}
    >
      <span className="flex items-center gap-1.5">
        <span
          className={cn(
            "size-1.5 shrink-0 rounded-full",
            (running || starting) && "animate-live bg-deck-live",
            blocked && "bg-deck-attention",
            done && "bg-deck-done",
            !starting && !running && !blocked && !done && "border border-deck-faint/60",
          )}
        />
        <span className="font-mono text-[9.5px] text-deck-faint">{task.id.slice(0, 8)}</span>
        {starting ? (
          <span className="ml-auto font-mono text-[9px] text-deck-live">starting</span>
        ) : (
          task.objective_gate && (
            <span className="ml-auto font-mono text-[9px] text-deck-faint">gate</span>
          )
        )}
      </span>
      <span
        className={cn(
          "line-clamp-2 text-[11.5px] leading-[15px]",
          done ? "text-deck-dim" : "text-deck-text",
        )}
      >
        {task.title}
      </span>
    </button>
  );
}

/**
 * Groups tasks into columns by longest dependency depth.
 *
 * Longest rather than shortest path: a task must sit to the right of *everything* it depends on,
 * and taking the shortest would put a node left of one of its own prerequisites.
 *
 * Iterative with a visited set rather than plain recursion — a cycle here would hang the render,
 * and while the planner rejects cycles, a graph arriving from anywhere else must not be able to
 * lock the UI.
 */
function depthColumns(tasks: TaskSummary[], edges: GraphEdge[]): TaskSummary[][] {
  const dependsOn = new Map<string, string[]>();
  for (const edge of edges) {
    dependsOn.set(edge.to, [...(dependsOn.get(edge.to) ?? []), edge.from]);
  }

  const depth = new Map<string, number>();
  const resolve = (id: string, seen: Set<string>): number => {
    if (depth.has(id)) return depth.get(id)!;
    if (seen.has(id)) return 0;
    seen.add(id);
    const parents = dependsOn.get(id) ?? [];
    const d = parents.length === 0 ? 0 : Math.max(...parents.map((p) => resolve(p, seen) + 1));
    depth.set(id, d);
    return d;
  };

  for (const task of tasks) resolve(task.id, new Set());

  const max = Math.max(0, ...tasks.map((t) => depth.get(t.id) ?? 0));
  const columns: TaskSummary[][] = Array.from({ length: max + 1 }, () => []);
  for (const task of tasks) columns[depth.get(task.id) ?? 0].push(task);
  return columns.filter((c) => c.length > 0);
}
