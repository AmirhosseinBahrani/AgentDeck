import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  CircleDollarSign,
  GitBranch,
  Play,
  ShieldAlert,
  Square,
  Terminal,
} from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Empty, Panel } from "../../components/ui/panel";
import { StatusDot } from "../../components/ui/status-dot";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../components/ui/tooltip";
import type { Autonomy, RunSnapshot, TaskSummary } from "../../lib/types";
import { cn } from "../../lib/utils";
import { AutonomyPicker, AutonomyStripe } from "./AutonomyPicker";

/**
 * The dashboard the product is built around.
 *
 * Deliberately the default view rather than a chat: AgentDeck is Objective → Run → Tasks →
 * Agents → Sessions, and a transcript is one way to observe a session rather than the thing
 * itself. Leading with the org state is what keeps that true as the UI grows.
 *
 * Three columns because they answer three different questions, in the order an operator asks
 * them: what is happening, what needs me, and why did it decide that.
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
      setError(stopped > 0 ? `Stopped ${stopped} agent${stopped === 1 ? "" : "s"}.` : null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

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

  const running = !!(snapshot?.active && snapshot.phase !== "");
  const effectiveMode = running ? (snapshot?.autonomy ?? autonomy) : autonomy;
  const tasks = snapshot?.tasks ?? [];
  const held = tasks.filter((t) => t.awaiting_approval);
  const live = tasks.filter((t) => t.status === "running").length;

  return (
    <div className="flex h-full min-h-0 flex-col">
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
        <div className="shrink-0 border-b border-deck-attention/25 bg-deck-attention/8 px-4 py-1.5 text-[11px] text-deck-attention">
          {error}
        </div>
      )}

      <div className="grid min-h-0 flex-1 grid-cols-[1.1fr_0.95fr_1fr] divide-x divide-white/6">
        <Panel
          title="Tasks"
          count={tasks.length}
          accent={live > 0 ? "live" : undefined}
          className="animate-rise"
        >
          {tasks.length === 0 ? (
            <Empty>
              No tasks yet. Give the team an objective and the supervisor will decompose it into a
              task graph.
            </Empty>
          ) : (
            tasks.map((task, i) => (
              <TaskRow key={task.id} task={task} index={i} onForceKill={forceKill} />
            ))
          )}
        </Panel>

        <Panel
          title={held.length > 0 ? "Waiting for you" : "Blockers"}
          count={held.length}
          accent={held.length > 0 ? "attention" : undefined}
          className="animate-rise [animation-delay:60ms]"
        >
          {held.length > 0 ? (
            held.map((task) => <ApprovalRow key={task.id} task={task} onApprove={approve} />)
          ) : (
            <Blockers snapshot={snapshot} />
          )}
        </Panel>

        <Panel
          title="Decisions"
          count={snapshot?.decisions.length}
          className="animate-rise [animation-delay:120ms]"
        >
          {!snapshot?.decisions.length ? (
            <Empty>
              The supervisor has not decided anything yet. Every choice it makes lands here with
              who made it — code, Claude, or you.
            </Empty>
          ) : (
            snapshot.decisions.map((d, i) => (
              <div key={i} className="card-row px-2 py-1.5 text-[11px]">
                <div className="flex items-baseline gap-1.5">
                  <DecidedBy by={d.decided_by} />
                  <span className="text-deck-dim">{d.stage}</span>
                  <span className="font-mono text-[10px] text-deck-faint">#{d.iteration}</span>
                  {d.repaired && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <span className="ml-auto">
                          <Badge tone="attention">repaired</Badge>
                        </span>
                      </TooltipTrigger>
                      <TooltipContent>
                        The model's first answer failed validation and had to be corrected before
                        it was allowed to touch state.
                      </TooltipContent>
                    </Tooltip>
                  )}
                </div>
                {d.rationale && (
                  <div className="mt-1 leading-relaxed text-deck-faint">{d.rationale}</div>
                )}
              </div>
            ))
          )}
        </Panel>
      </div>

      <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-white/6 px-4 font-mono text-[10px] text-deck-faint">
        <Stat icon={<Activity className="size-3" />} label="iter" value={snapshot?.iteration ?? 0} />
        <Stat
          icon={<CircleDollarSign className="size-3" />}
          label="spent"
          value={`$${(snapshot?.spent_usd ?? 0).toFixed(4)}`}
        />
        <Stat
          icon={<GitBranch className="size-3" />}
          label="merged"
          value={snapshot?.integrated ? "yes" : "not yet"}
          tone={snapshot?.integrated ? "done" : undefined}
        />
        {(snapshot?.open_escalations ?? 0) > 0 && (
          <Stat
            icon={<ShieldAlert className="size-3" />}
            label="awaiting you"
            value={snapshot?.open_escalations ?? 0}
            tone="attention"
          />
        )}
        <Button variant="ghost" size="sm" className="ml-auto font-mono" onClick={onOpenSession}>
          <Terminal /> transcripts
        </Button>
      </footer>
    </div>
  );
}

function Stat({
  icon,
  label,
  value,
  tone,
}: {
  icon: React.ReactNode;
  label: string;
  value: React.ReactNode;
  tone?: "attention" | "done";
}) {
  return (
    <span
      className={cn(
        "flex items-center gap-1.5",
        tone === "attention" && "text-deck-attention",
        tone === "done" && "text-deck-done",
      )}
    >
      {icon}
      <span className="text-deck-faint">{label}</span>
      <span className={cn(!tone && "text-deck-dim")}>{value}</span>
    </span>
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
    <header className="glass-flat shrink-0 border-b border-white/8 px-4 py-3">
      <div className="label-micro mb-1.5">Objective</div>

      {snapshot?.objective ? (
        // The largest thing on the page while a run is live: everything else is in service of it.
        <div className="mb-2.5 text-[15px] leading-snug font-medium text-deck-text">
          {snapshot.objective}
        </div>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (objective.trim() && !busy) onStart();
          }}
          className="mb-2.5"
        >
          {/* Submits on Enter. Typing an objective and pressing return is the most likely first
              action anyone takes, and making them reach for the mouse to do it is rude. */}
          <Input
            value={objective}
            onChange={(e) => onObjectiveChange(e.target.value)}
            placeholder="Build a production-ready webhook retry system for the payments service"
            className="text-[14px]"
            autoFocus
          />
        </form>
      )}

      <div className="flex items-center gap-3">
        <PhasePill phase={snapshot?.phase} running={running} />

        {/* Locked while a run is live: the supervisor enforces the mode it was started with, so
            an editable control here would claim a change that never reached it. */}
        <AutonomyPicker value={autonomy} onChange={onAutonomyChange} disabled={running} />

        <div className="ml-auto flex items-center gap-2">
          {running ? (
            <Button variant="danger" size="md" onClick={onCancel} disabled={busy}>
              <Square /> Stop run
            </Button>
          ) : (
            <>
              {!objective.trim() && (
                // Stated on screen, not only in a tooltip. A disabled control with no visible
                // reason is indistinguishable from a broken one.
                <span className="text-[11px] text-deck-faint">
                  Describe what the team should build.
                </span>
              )}
              <Button
                variant="primary"
                size="md"
                onClick={onStart}
                disabled={busy || !objective.trim()}
              >
                <Play /> {busy ? "Starting…" : "Start run"}
              </Button>
            </>
          )}
        </div>
      </div>
    </header>
  );
}

function PhasePill({ phase, running }: { phase?: string; running: boolean }) {
  if (!phase) {
    return <Badge tone="neutral">idle</Badge>;
  }
  // Blocked is the only phase that demands attention, so it is the only coloured one. Colouring
  // every phase would make none of them stand out.
  if (phase === "blockedonhuman") {
    return <Badge tone="attention">waiting on you</Badge>;
  }
  return (
    <Badge tone={running ? "live" : "neutral"}>
      {running && <span className="animate-live size-1.5 rounded-full bg-current" />}
      {phase}
    </Badge>
  );
}

function TaskRow({
  task,
  index,
  onForceKill,
}: {
  task: TaskSummary;
  index: number;
  onForceKill: (taskId: string) => void;
}) {
  const [confirming, setConfirming] = useState(false);

  return (
    <div
      className="card-row animate-rise px-2 py-1.5"
      style={{ animationDelay: `${Math.min(index, 8) * 25}ms` }}
    >
      <div className="flex items-center gap-2">
        <StatusDot status={task.status} />
        <span className="min-w-0 flex-1 truncate text-[12px] text-deck-text">{task.title}</span>

        {/* Only on a running task, and behind a confirm: the kill is immediate and unconditional
            once clicked. The work survives; the turn in progress does not. */}
        {task.status === "running" &&
          (confirming ? (
            <span className="flex items-center gap-1">
              <Button
                variant="danger"
                size="sm"
                onClick={() => {
                  setConfirming(false);
                  onForceKill(task.id);
                }}
              >
                Kill now
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
                Cancel
              </Button>
            </span>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button variant="ghost" size="sm" onClick={() => setConfirming(true)}>
                  Stop
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                Stops this agent immediately. Its worktree and changes are kept, and the task is
                cancelled rather than failed — so it does not consume a retry.
              </TooltipContent>
            </Tooltip>
          ))}

        {task.objective_gate && (
          <Tooltip>
            <TooltipTrigger asChild>
              <span>
                <Badge tone="neutral">gate</Badge>
              </span>
            </TooltipTrigger>
            <TooltipContent>The objective cannot complete without this task.</TooltipContent>
          </Tooltip>
        )}
      </div>

      <div className="mt-1 flex items-center gap-2 pl-3.5 font-mono text-[10px] text-deck-faint">
        <span>{task.status}</span>
        {task.role && <span>· {task.role}</span>}
        {task.attempts > 0 && <span>· try {task.attempts}</span>}
        {task.review_rounds > 0 && <span>· review {task.review_rounds}</span>}
      </div>

      {task.blocked_reason && (
        <div className="mt-1 pl-3.5 text-[10px] leading-relaxed text-deck-attention/85">
          {task.blocked_reason}
        </div>
      )}
    </div>
  );
}

/**
 * An agent the supervisor is ready to start but is not allowed to.
 *
 * Given the middle column rather than a row action, because in assisted and manual modes this is
 * the run's critical path — everything else is waiting on it, and a run that looks idle when it
 * is actually waiting for a click is the worst version of this feature.
 */
function ApprovalRow({
  task,
  onApprove,
}: {
  task: TaskSummary;
  onApprove: (taskId: string) => void;
}) {
  return (
    <div className="animate-rise rounded-[7px] border border-deck-attention/25 bg-deck-attention/6 px-2 py-1.5">
      <div className="text-[12px] text-deck-text">{task.title}</div>
      <div className="mt-1 flex items-center gap-2 font-mono text-[10px] text-deck-faint">
        <span>{task.role || "unassigned"}</span>
        {task.attempts > 0 && <span>· attempt {task.attempts + 1}</span>}
        <Button variant="attention" size="sm" className="ml-auto" onClick={() => onApprove(task.id)}>
          <Play /> Start agent
        </Button>
      </div>
    </div>
  );
}

function Blockers({ snapshot }: { snapshot: RunSnapshot | null }) {
  const blocked = snapshot?.tasks.filter((t) => t.status === "blocked" || t.blocked_reason) ?? [];

  if (snapshot?.phase === "blockedonhuman") {
    return (
      <div className="rounded-[7px] border border-deck-attention/30 bg-deck-attention/8 px-2.5 py-2 text-[12px] leading-relaxed text-deck-attention">
        The run is waiting for a decision from you. It will resume once you answer.
      </div>
    );
  }
  if (!blocked.length) {
    return <Empty>Nothing is blocked. Anything needing your decision will appear here.</Empty>;
  }
  return (
    <>
      {blocked.map((task) => (
        <div
          key={task.id}
          className="rounded-[7px] border border-deck-attention/25 bg-deck-attention/6 px-2 py-1.5 text-[11px]"
        >
          <div className="text-deck-text">{task.title}</div>
          {task.blocked_reason && (
            <div className="mt-0.5 leading-relaxed text-deck-attention/85">
              {task.blocked_reason}
            </div>
          )}
        </div>
      ))}
    </>
  );
}

function DecidedBy({ by }: { by: string }) {
  // The most useful column on the page: it answers whether the model was actually driving, or
  // whether code kept falling back because its answers were unusable.
  return (
    <span
      className={cn(
        "font-mono text-[10px]",
        by === "claude" && "text-deck-live",
        by === "human" && "text-deck-attention",
        by === "code" && "text-deck-faint",
      )}
    >
      {by}
    </span>
  );
}
