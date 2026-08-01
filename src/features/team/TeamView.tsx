import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import type { Autonomy, RunSnapshot, TaskSummary } from "../../lib/types";
import { AutonomyPicker, AutonomyStripe } from "./AutonomyPicker";

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
  // Assisted until the operator says otherwise. Once a run starts, the mode it was started with
  // is authoritative — the picker reflects the run rather than a local preference that no longer
  // matches what the supervisor is actually enforcing.
  const [autonomy, setAutonomy] = useState<Autonomy>("assisted");

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
      await invoke("start_supervisor_run", { objective, maxCostUsd: 5.0, autonomy });
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

  const running = !!(snapshot?.active && snapshot.phase !== "");
  const effectiveMode = running ? (snapshot?.autonomy ?? autonomy) : autonomy;
  const held = snapshot?.tasks.filter((t) => t.awaiting_approval) ?? [];

  async function forceKill(taskId: string) {
    try {
      const killed = await invoke<boolean>("force_kill_agent", { taskId });
      setError(killed ? null : "That agent was already gone.");
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  async function approve(taskId: string) {
    try {
      await invoke("approve_dispatch", { taskId });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Ambient, because "can agents act without asking me" is a question the operator needs
          answered while looking at something else. */}
      <AutonomyStripe mode={effectiveMode} active={running} />

      <ObjectiveHeader
        objective={objective}
        onObjectiveChange={setObjective}
        snapshot={snapshot}
        running={running}
        busy={busy}
        onStart={start}
        onCancel={cancel}
        autonomy={effectiveMode}
        onAutonomyChange={setAutonomy}
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
                <TaskRow key={task.id} task={task} onForceKill={forceKill} />
              ))}
            </ul>
          )}
        </Panel>

        <Panel title={held.length > 0 ? `Waiting for you (${held.length})` : "Blockers"}>
          {held.length > 0 ? (
            <Approvals tasks={held} onApprove={approve} />
          ) : (
            <Blockers snapshot={snapshot} />
          )}
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
        {/* Stated explicitly, because all-green tasks look like a finished run and are not one
            until the branches have been merged and tested together. */}
        <span className={snapshot?.integrated ? "text-emerald-500" : undefined}>
          {snapshot?.integrated ? "branches integrate" : "not yet integrated"}
        </span>
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
  autonomy,
  onAutonomyChange,
}: {
  objective: string;
  onObjectiveChange: (value: string) => void;
  snapshot: RunSnapshot | null;
  running: boolean;
  busy: boolean;
  onStart: () => void;
  onCancel: () => void;
  autonomy: Autonomy;
  onAutonomyChange: (mode: Autonomy) => void;
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

        {/* Locked while a run is live: the supervisor enforces the mode it was started with, so
            an editable control here would claim a change that never reached it. */}
        <AutonomyPicker value={autonomy} onChange={onAutonomyChange} disabled={running} />

        {running ? (
          <button
            onClick={onCancel}
            disabled={busy}
            className="rounded border border-red-900/70 px-2.5 py-1 text-[12px] text-red-300 hover:bg-red-950/40 disabled:opacity-50"
          >
            Stop run
          </button>
        ) : (
          // Disabled only on an empty objective. The previous eight-character floor was
          // arbitrary — "fix CI" is a legitimate objective — and it greyed the button out with
          // nothing on screen explaining why, which reads as the app being broken.
          <button
            onClick={onStart}
            disabled={busy || !objective.trim()}
            title={objective.trim() ? undefined : "Describe what the team should build first"}
            className="rounded bg-neutral-100 px-2.5 py-1 text-[12px] font-medium text-neutral-900 hover:bg-white disabled:opacity-40"
          >
            {busy ? "Starting…" : "Start run"}
          </button>
        )}

        {!running && !objective.trim() && (
          // Said on screen, not only in a tooltip. A disabled control with no stated reason is
          // indistinguishable from a broken one.
          <span className="text-[11px] text-neutral-600">
            Describe what the team should build.
          </span>
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

function TaskRow({
  task,
  onForceKill,
}: {
  task: TaskSummary;
  onForceKill: (taskId: string) => void;
}) {
  const [confirming, setConfirming] = useState(false);

  return (
    <li className="rounded border border-neutral-800 px-2 py-1.5">
      <div className="flex items-center gap-2">
        <StatusDot status={task.status} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-neutral-200">{task.title}</span>
        {/* Only on a running task, and behind a confirm. Killing is immediate and unconditional
            once clicked, so the confirm is the only thing between a stray click and a stopped
            agent — the work survives, but the turn in progress does not. */}
        {task.status === "running" &&
          (confirming ? (
            <span className="flex items-center gap-1">
              <button
                onClick={() => {
                  setConfirming(false);
                  onForceKill(task.id);
                }}
                className="rounded bg-red-900/70 px-1.5 py-0.5 text-[10px] text-red-100 hover:bg-red-800"
              >
                Kill now
              </button>
              <button
                onClick={() => setConfirming(false)}
                className="text-[10px] text-neutral-500 hover:text-neutral-300"
              >
                Cancel
              </button>
            </span>
          ) : (
            <button
              onClick={() => setConfirming(true)}
              title="Stops this agent immediately. Its worktree and changes are kept, and the task is cancelled rather than failed."
              className="text-[10px] text-neutral-600 hover:text-red-400"
            >
              Stop
            </button>
          ))}
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

/**
 * Agents the supervisor is ready to start but is not allowed to.
 *
 * Given its own panel rather than a row action, because in assisted and manual modes this is
 * the run's critical path — everything else is waiting on it, and a run that looks idle when it
 * is actually waiting for a click is the worst version of this feature.
 */
function Approvals({
  tasks,
  onApprove,
}: {
  tasks: TaskSummary[];
  onApprove: (taskId: string) => void;
}) {
  return (
    <ul className="space-y-1">
      {tasks.map((task) => (
        <li
          key={task.id}
          className="rounded border border-sky-900/60 bg-sky-950/20 px-2 py-1.5 text-[11px]"
        >
          <div className="text-neutral-200">{task.title}</div>
          <div className="mt-0.5 flex items-center gap-2 text-[10px] text-neutral-500">
            <span>{task.role || "unassigned"}</span>
            {task.attempts > 0 && <span>· attempt {task.attempts + 1}</span>}
            <button
              onClick={() => onApprove(task.id)}
              className="ml-auto rounded bg-neutral-100 px-1.5 py-0.5 text-[10px] font-medium text-neutral-900 hover:bg-white"
            >
              Start agent
            </button>
          </div>
        </li>
      ))}
    </ul>
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
