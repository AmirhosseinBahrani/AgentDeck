import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import type { RunSnapshot, TaskSummary } from "../../lib/types";

/**
 * The dashboard the product is built around.
 *
 * Deliberately the default view rather than a chat: the spec's central point is that AgentDeck is
 * Objective → Run → Tasks → Agents → Sessions, and a transcript is one way to observe a session
 * rather than the thing itself. Leading with the org state is what keeps that true as the UI grows.
 */
export function TeamView({ onOpenSession }: { onOpenSession: () => void }) {
  const [objective, setObjective] = useState("");
  const [snapshot, setSnapshot] = useState<RunSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await invoke<RunSnapshot>("get_run_snapshot"));
    } catch {
      // A failed poll is not worth surfacing; the next one will succeed or the run has ended.
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Polled rather than event-driven: the snapshot changes once per supervisor iteration, which
    // is far slower than the event stream, and a second of staleness costs nothing here.
    const id = setInterval(() => void refresh(), 1000);
    return () => clearInterval(id);
  }, [refresh]);

  async function start() {
    setBusy(true);
    setError(null);
    try {
      await invoke("start_supervisor_run", { objective, maxCostUsd: 5.0 });
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function cancel() {
    setBusy(true);
    try {
      const stopped = await invoke<number>("cancel_supervisor_run");
      setError(stopped > 0 ? `Stopped ${stopped} agent(s).` : null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const running = snapshot?.active && snapshot.phase !== "";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ObjectiveHeader
        objective={objective}
        onObjectiveChange={setObjective}
        snapshot={snapshot}
        running={!!running}
        busy={busy}
        onStart={start}
        onCancel={cancel}
      />

      {error && (
        <div className="border-b border-amber-800/60 bg-amber-950/30 px-4 py-1.5 text-[11px] text-amber-300">
          {error}
        </div>
      )}

      <div className="grid min-h-0 flex-1 grid-cols-[1fr_1fr_1fr] divide-x divide-neutral-800">
        <Panel title="Tasks">
          {!snapshot?.tasks.length ? (
            <Empty>No tasks yet. Start a run to have the supervisor plan some.</Empty>
          ) : (
            <ul className="space-y-1">
              {snapshot.tasks.map((task) => (
                <TaskRow key={task.id} task={task} />
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="Blockers">
          <Blockers snapshot={snapshot} />
        </Panel>

        <Panel title="Decisions">
          {!snapshot?.decisions.length ? (
            <Empty>The supervisor has not decided anything yet.</Empty>
          ) : (
            <ul className="space-y-1.5">
              {snapshot.decisions.map((d, i) => (
                <li key={i} className="text-[11px]">
                  <div className="flex items-baseline gap-1.5">
                    <DecidedBy by={d.decided_by} />
                    <span className="text-neutral-400">{d.stage}</span>
                    <span className="text-neutral-600">#{d.iteration}</span>
                    {d.repaired && (
                      <span
                        className="text-amber-500"
                        title="The model's first answer failed validation and had to be corrected"
                      >
                        repaired
                      </span>
                    )}
                  </div>
                  {d.rationale && (
                    <div className="mt-0.5 text-neutral-500">{d.rationale}</div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </div>

      <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-neutral-800 px-4 text-[10px] text-neutral-600">
        <span>Iteration {snapshot?.iteration ?? 0}</span>
        <span>${(snapshot?.spent_usd ?? 0).toFixed(4)} spent</span>
        <span className={snapshot?.open_escalations ? "text-amber-400" : undefined}>
          {snapshot?.open_escalations ?? 0} awaiting you
        </span>
        <button onClick={onOpenSession} className="ml-auto hover:text-neutral-300">
          Session transcripts →
        </button>
      </footer>
    </div>
  );
}

function ObjectiveHeader({
  objective,
  onObjectiveChange,
  snapshot,
  running,
  busy,
  onStart,
  onCancel,
}: {
  objective: string;
  onObjectiveChange: (value: string) => void;
  snapshot: RunSnapshot | null;
  running: boolean;
  busy: boolean;
  onStart: () => void;
  onCancel: () => void;
}) {
  return (
    <header className="shrink-0 border-b border-neutral-800 px-4 py-3">
      <div className="mb-1 text-[10px] tracking-wider text-neutral-500 uppercase">Objective</div>

      {snapshot?.objective ? (
        // The objective is the largest thing on the page while a run is live: everything else is
        // in service of it.
        <div className="mb-2 text-[15px] text-neutral-100">{snapshot.objective}</div>
      ) : (
        <input
          value={objective}
          onChange={(e) => onObjectiveChange(e.target.value)}
          placeholder="Build a production-ready webhook retry system for the payments service"
          className="mb-2 w-full rounded border border-neutral-700 bg-neutral-900 px-2.5 py-1.5 text-[13px] text-neutral-100 placeholder:text-neutral-600 focus:border-neutral-500 focus:outline-none"
        />
      )}

      <div className="flex items-center gap-3">
        <PhasePill phase={snapshot?.phase} />

        {running ? (
          <button
            onClick={onCancel}
            disabled={busy}
            className="rounded border border-red-900/70 px-2.5 py-1 text-[12px] text-red-300 hover:bg-red-950/40 disabled:opacity-50"
          >
            Stop run
          </button>
        ) : (
          <button
            onClick={onStart}
            disabled={busy || objective.trim().length < 8}
            className="rounded bg-neutral-100 px-2.5 py-1 text-[12px] font-medium text-neutral-900 hover:bg-white disabled:opacity-40"
          >
            Start run
          </button>
        )}
      </div>
    </header>
  );
}

function PhasePill({ phase }: { phase?: string }) {
  if (!phase) {
    return <span className="text-[11px] text-neutral-600">idle</span>;
  }
  // Blocked is the only phase that demands attention, so it is the only coloured one — colouring
  // everything would make nothing stand out.
  const blocked = phase === "blockedonhuman";
  return (
    <span
      className={`rounded px-1.5 py-0.5 text-[11px] ${
        blocked ? "bg-amber-950/60 text-amber-300" : "bg-neutral-800 text-neutral-400"
      }`}
    >
      {blocked ? "waiting on you" : phase}
    </span>
  );
}

function TaskRow({ task }: { task: TaskSummary }) {
  return (
    <li className="rounded border border-neutral-800 px-2 py-1.5">
      <div className="flex items-center gap-2">
        <StatusDot status={task.status} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-neutral-200">{task.title}</span>
        {task.objective_gate && (
          <span
            className="text-[10px] text-neutral-500"
            title="The objective cannot complete without this task"
          >
            gate
          </span>
        )}
      </div>
      <div className="mt-0.5 flex items-center gap-2 pl-4 text-[10px] text-neutral-500">
        <span>{task.status}</span>
        {task.role && <span>· {task.role}</span>}
        {task.attempts > 0 && <span>· attempt {task.attempts}</span>}
        {task.review_rounds > 0 && <span>· review {task.review_rounds}</span>}
      </div>
      {task.blocked_reason && (
        <div className="mt-1 pl-4 text-[10px] text-amber-400/80">{task.blocked_reason}</div>
      )}
    </li>
  );
}

/** Shape and colour, never colour alone. */
function StatusDot({ status }: { status: string }) {
  const style: Record<string, string> = {
    running: "bg-emerald-400 animate-pulse",
    review: "bg-sky-400",
    completed: "bg-neutral-500",
    failed: "bg-red-500",
    cancelled: "bg-neutral-700",
    blocked: "bg-amber-400",
    assigned: "bg-neutral-400",
    queued: "border border-neutral-600",
    backlog: "border border-neutral-700",
  };
  return (
    <span
      className={`h-1.5 w-1.5 shrink-0 rounded-full ${style[status] ?? "bg-neutral-600"}`}
      title={status}
    />
  );
}

function Blockers({ snapshot }: { snapshot: RunSnapshot | null }) {
  const blocked = snapshot?.tasks.filter((t) => t.status === "blocked" || t.blocked_reason) ?? [];

  if (snapshot?.phase === "blockedonhuman") {
    return (
      <div className="rounded border border-amber-800/70 bg-amber-950/30 px-2.5 py-2 text-[12px] text-amber-200">
        The run is waiting for a decision from you. It will resume once you answer.
      </div>
    );
  }
  if (!blocked.length) {
    return <Empty>Nothing is blocked.</Empty>;
  }
  return (
    <ul className="space-y-1">
      {blocked.map((task) => (
        <li key={task.id} className="rounded border border-amber-900/50 px-2 py-1.5 text-[11px]">
          <div className="text-neutral-200">{task.title}</div>
          {task.blocked_reason && (
            <div className="mt-0.5 text-amber-400/80">{task.blocked_reason}</div>
          )}
        </li>
      ))}
    </ul>
  );
}

function DecidedBy({ by }: { by: string }) {
  // The most useful column on the page: it answers whether the model was actually driving, or
  // whether code kept falling back because its answers were unusable.
  const style =
    by === "claude"
      ? "text-sky-400"
      : by === "human"
        ? "text-amber-400"
        : "text-neutral-500";
  return <span className={`font-mono ${style}`}>{by}</span>;
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex min-h-0 flex-col">
      <h2 className="shrink-0 border-b border-neutral-800 px-3 py-1.5 text-[10px] tracking-wider text-neutral-500 uppercase">
        {title}
      </h2>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">{children}</div>
    </section>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return <p className="px-1 text-[11px] text-neutral-600">{children}</p>;
}
