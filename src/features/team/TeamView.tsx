import { invoke } from "@tauri-apps/api/core";
import { History, Loader2, Play, RotateCcw, Square, UserPlus } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { SectionRule } from "../../components/ui/section-rule";
import type {
  Autonomy,
  EscalationAnswer,
  PastRunSummary,
  RunSnapshot,
} from "../../lib/types";
import { cn } from "../../lib/utils";
import { AgentRoster } from "./AgentRoster";
import { AutonomyPicker } from "./AutonomyPicker";
import { EscalationInbox } from "./EscalationInbox";
import { HireAgent } from "./HireAgent";
import { ProjectSwitcher } from "./ProjectSwitcher";
import { RevokeAgent } from "./RevokeAgent";
import { TaskGraph } from "./TaskGraph";

const SUPERVISOR_LOOP = ["observe", "plan", "assign", "review", "escalate"] as const;

/**
 * The default screen: the state of the team, not a chat.
 *
 * Laid out as the operator's questions in the order they ask them — what are we doing, how far
 * along is it, who is working, what is the plan, and what needs me. The right column is reserved
 * entirely for the last of those, because a decision waiting on a human is the only thing on
 * screen that stops everything else.
 */
export function TeamView({
  onOpenSession,
  projectPath,
  onProjectChanged,
}: {
  onOpenSession: (sessionId?: string) => void;
  projectPath: string | null;
  onProjectChanged: () => void;
}) {
  const [objective, setObjective] = useState("");
  const [snapshot, setSnapshot] = useState<RunSnapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [answering, setAnswering] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState<Set<string>>(() => new Set());
  const [autonomy, setAutonomy] = useState<Autonomy>("assisted");
  const [now, setNow] = useState(Date.now());
  const [hiring, setHiring] = useState(false);
  const [revoking, setRevoking] = useState<string | null>(null);

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
    const id = setInterval(() => {
      void refresh();
      setNow(Date.now());
    }, 1000);
    return () => clearInterval(id);
  }, [refresh]);

  const start = () => startWith(objective);

  // Takes the objective as an argument rather than reading state: a new project starts its first
  // run in the same tick the field is set, and state would still hold the previous value.
  async function startWith(text: string) {
    setBusy(true);
    setError(null);
    try {
      await invoke("start_supervisor_run", {
        objective: text,
        maxCostUsd: 5.0,
        autonomy,
      });
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

  async function answer(escalationId: string, choice: EscalationAnswer) {
    setAnswering(escalationId);
    setError(null);
    try {
      await invoke("answer_escalation", { escalationId, answer: choice });
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setAnswering(null);
    }
  }

  /**
 * Shown between pressing Start and the first task existing.
 *
 * That gap is one model call and can run to the better part of a minute, and until it returns
 * there is genuinely nothing to draw: no tasks, no agent doing anything. An empty graph is an
 * accurate picture of that state and a completely misleading one, because it is identical to a
 * run that failed to start. The elapsed count is what separates the two — it says the app is
 * still with you, and gives you the evidence to decide when it is not.
 */
function PlanningNotice({ startedAt }: { startedAt: number }) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const seconds = startedAt ? Math.max(0, Math.floor((now - startedAt) / 1000)) : 0;

  return (
    <div className="flex items-center gap-3 rounded-[var(--radius-panel)] border border-white/[0.07] bg-white/[0.025] px-4 py-3.5">
      <Loader2 className="size-4 shrink-0 animate-spin text-deck-live" />
      <div className="flex min-w-0 grow flex-col gap-0.5">
        <span className="text-[12.5px] font-medium text-deck-text">
          Breaking the objective into tasks
        </span>
        <span className="text-[11.5px] leading-relaxed text-deck-faint">
          One planning call to Claude. Agents start the moment it returns — usually under a
          minute.
        </span>
      </div>
      <span className="shrink-0 font-mono text-[11px] tabular-nums text-deck-faint">
        {seconds}s
      </span>
    </div>
  );
}

/** Returns to the start screen, which is also where earlier runs are listed. */
  async function newRun() {
    try {
      await invoke("clear_run");
      setObjective("");
      setSnapshot(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  /**
   * Grants one dispatch, and shows that it was granted.
   *
   * The supervisor applies approvals on its next iteration, so between the click and the agent
   * actually starting there is a gap of up to a tick in which the task looks exactly as it did
   * before. Without a local marker the button reads as broken and gets pressed again.
   */
  async function approve(taskId: string) {
    setStarting((prev) => new Set(prev).add(taskId));
    try {
      await invoke("approve_dispatch", { taskId });
      await refresh();
    } catch (e) {
      setError(String(e));
      setStarting((prev) => {
        const next = new Set(prev);
        next.delete(taskId);
        return next;
      });
    }
  }

  // Cleared by the snapshot rather than by a timer: the marker exists to cover the wait for the
  // supervisor, so the supervisor having acted is exactly when it should go.
  useEffect(() => {
    if (starting.size === 0) return;
    const stillHeld = new Set(
      (snapshot?.tasks ?? []).filter((t) => t.awaiting_approval).map((t) => t.id),
    );
    setStarting((prev) => {
      const next = new Set([...prev].filter((id) => stillHeld.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [snapshot, starting.size]);

  const running = !!(snapshot?.active && snapshot.phase !== "");
  const tasks = snapshot?.tasks ?? [];
  const escalations = snapshot?.escalations ?? [];
  const held = tasks.filter((t) => t.awaiting_approval);
  const counts = {
    done: tasks.filter((t) => t.status === "completed").length,
    running: tasks.filter((t) => t.status === "running" || t.status === "review").length,
    blocked: tasks.filter((t) => t.status === "blocked").length,
    queued: tasks.filter((t) => ["queued", "assigned", "backlog"].includes(t.status)).length,
  };

  if (!running && !snapshot?.objective) {
    return (
      <StartScreen
        objective={objective}
        onObjectiveChange={setObjective}
        onStart={start}
        busy={busy}
        autonomy={autonomy}
        onAutonomyChange={setAutonomy}
        error={error}
      />
    );
  }

  const blocked = snapshot?.phase === "blockedonhuman";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <HireAgent open={hiring} onClose={() => setHiring(false)} onHired={() => void refresh()} />
      <RevokeAgent
        agent={snapshot?.agents.find((a) => a.id === revoking) ?? null}
        onClose={() => setRevoking(null)}
        onRevoked={() => void refresh()}
      />


      <header className="flex shrink-0 items-end gap-[60px] border-b border-white/[0.07] px-7 pt-[22px] pb-[18px]">
        <div className="flex min-w-0 grow flex-col gap-3">
          <div className="flex items-center gap-3">
            <span className="label-micro">Objective</span>
            <span
              className={cn(
                "flex h-5 items-center gap-[7px] rounded-full px-[9px]",
                blocked ? "bg-deck-attention/[0.13]" : "bg-deck-live/[0.13]",
              )}
            >
              <span
                className={cn(
                  "size-[5px] rounded-full",
                  blocked ? "bg-deck-attention" : "animate-live bg-deck-live",
                )}
              />
              <span
                className={cn(
                  "font-mono text-[10px] font-medium tracking-[0.06em] uppercase",
                  blocked ? "text-deck-attention" : "text-deck-live",
                )}
              >
                supervisor · iteration {snapshot?.iteration ?? 0} · {snapshot?.phase}
              </span>
            </span>
          </div>
          <h1 className="text-[28px] leading-[34px] font-semibold tracking-[-0.025em] text-deck-text">
            {snapshot?.objective}
          </h1>
          <p className="text-[13px] leading-5 text-deck-dim">
            {[
              `${elapsed(snapshot?.started_at_ms, now)} elapsed`,
              `$${(snapshot?.spent_usd ?? 0).toFixed(2)} spent`,
              escalations.length > 0
                ? `${escalations.length} decision${escalations.length === 1 ? "" : "s"} waiting on you`
                : held.length > 0
                  ? `${held.length} agent${held.length === 1 ? "" : "s"} waiting to start`
                  : snapshot?.integrated
                    ? "branches merge cleanly"
                    : "branches not yet merged",
            ].join(" · ")}
          </p>
        </div>

        <div className="flex shrink-0 items-end gap-[30px]">
          <Counter value={counts.done} label="done" tone="done" />
          <Counter value={counts.running} label="running" tone="live" />
          <Counter value={counts.blocked} label="blocked" tone="attention" />
          <Counter value={counts.queued} label="queued" />
          {running ? (
            <Button variant="danger" size="lg" onClick={cancel} disabled={busy}>
              <Square /> Stop run
            </Button>
          ) : (
            // The run is over. The useful action is starting another, not restarting this one —
            // and this is the only route back to the start screen, where earlier runs are listed.
            <Button variant="primary" size="lg" onClick={newRun} disabled={busy}>
              <RotateCcw /> New run
            </Button>
          )}
        </div>
      </header>

      {error && (
        <div className="shrink-0 border-b border-deck-attention/25 bg-deck-attention/[0.08] px-7 py-1.5 text-[11px] text-deck-attention">
          {error}
        </div>
      )}

      <div className="flex min-h-0 grow gap-[26px] px-7 pt-[18px] pb-[26px]">
        <div className="flex min-w-0 grow flex-col gap-[22px] overflow-y-auto">
          <ProjectSwitcher
            activePath={projectPath}
            onSwitched={() => {
              onProjectChanged();
              void refresh();
            }}
            onStarted={(firstObjective) => {
              setObjective(firstObjective);
              void startWith(firstObjective);
            }}
            runActive={running}
          />

          <section className="flex min-h-0 flex-col gap-1.5">
            <SectionRule
              label="Team"
              trailing={`max ${snapshot?.max_concurrent ?? 0} concurrent`}
              className="pb-2"
            />
            {/* Scrolls within its own height rather than growing the column. A roster of eight
                pushed the task graph off the bottom of the page — and the graph is the thing you
                look at to see whether the run is progressing, so the roster growing must not be
                what hides it. */}
            <div className="flex max-h-[38vh] min-h-0 flex-col overflow-y-auto pr-1">
              <AgentRoster
                agents={snapshot?.agents ?? []}
                onOpenSession={(id) => onOpenSession(id)}
                onRevoke={setRevoking}
              />
            </div>
            <Button
              variant="ghost"
              size="sm"
              className="mt-1 self-start"
              onClick={() => setHiring(true)}
            >
              <UserPlus /> Hire an agent
            </Button>
          </section>

          <section className="flex min-h-[180px] shrink-0 flex-col gap-3">
            <SectionRule
              label="Task graph"
              trailing={`${tasks.length} task${tasks.length === 1 ? "" : "s"}`}
            />
            {running && tasks.length === 0 ? (
              <PlanningNotice startedAt={snapshot?.started_at_ms ?? 0} />
            ) : (
              <TaskGraph
                tasks={tasks}
                edges={snapshot?.edges ?? []}
                starting={starting}
                onOpenSession={(id) => onOpenSession(id)}
              />
            )}
          </section>
        </div>

        <aside className="flex w-[372px] shrink-0 flex-col gap-[22px] overflow-y-auto">
          {escalations.length > 0 && (
            <EscalationInbox escalations={escalations} onAnswer={answer} busy={answering} />
          )}

          {held.map((task) => (
            <div
              key={task.id}
              className="rounded-[var(--radius-panel)] border border-deck-attention/30 bg-deck-attention/[0.08] p-4"
            >
              <div className="label-micro text-deck-attention">Waiting to start</div>
              <div className="mt-1.5 text-[13px] leading-5 text-deck-text">{task.title}</div>
              <Button
                variant="attention"
                size="md"
                className="mt-3 w-full"
                disabled={starting.has(task.id)}
                onClick={() => approve(task.id)}
              >
                {starting.has(task.id) ? (
                  <>
                    <Loader2 className="animate-spin" /> Starting…
                  </>
                ) : (
                  <>
                    <Play /> Start agent
                  </>
                )}
              </Button>
            </div>
          ))}

          <section className="flex flex-col gap-3">
            <SectionRule label="Supervisor loop" />
            <div className="flex flex-wrap items-center gap-1.5">
              {SUPERVISOR_LOOP.map((stage) => {
                // Highlights where the loop actually is. Without it these pills would be a
                // legend rather than an instrument — decoration that never changes.
                const active = currentStage(snapshot) === stage;
                return (
                  <span
                    key={stage}
                    className={cn(
                      "rounded-full px-[9px] py-1 font-mono text-[10.5px]",
                      active
                        ? stage === "escalate"
                          ? "border border-deck-attention/40 bg-deck-attention/[0.14] font-medium text-deck-attention"
                          : "border border-deck-live/40 bg-deck-live/[0.14] font-medium text-deck-live"
                        : "bg-white/[0.04] text-deck-faint",
                    )}
                  >
                    {stage}
                  </span>
                );
              })}
            </div>
          </section>

          <section className="flex min-h-0 grow flex-col gap-3">
            <SectionRule label="Recent decisions" />
            {!snapshot?.decisions.length ? (
              <p className="text-[11px] leading-relaxed text-deck-faint">
                Every choice the supervisor makes lands here with who made it — code, Claude, or
                you.
              </p>
            ) : (
              // Scrolls rather than grows. The rationale is model-written prose of no bounded
              // length, and without this the list painted straight over the section beneath it.
              <div className="flex min-h-0 grow flex-col gap-3 overflow-y-auto pr-1">
                {snapshot.decisions.slice(0, 12).map((d, i) => (
                  <div key={i} className="flex gap-[11px]">
                    <span className="w-[34px] shrink-0 pt-0.5 font-mono text-[10px] text-deck-faint">
                      #{d.iteration}
                    </span>
                    <div className="flex min-w-0 grow flex-col gap-0.5">
                      <span
                        className={cn(
                          "text-[12.5px] leading-[18px] font-medium",
                          i === 0 ? "text-deck-text" : "text-deck-dim",
                        )}
                      >
                        {d.kind.replace(/_/g, " ")}
                      </span>
                      {d.rationale && (
                        <span
                          title={d.rationale}
                          className="line-clamp-3 text-[11.5px] leading-[17px] break-words text-deck-faint"
                        >
                          {d.rationale}
                        </span>
                      )}
                      <span
                        className={cn(
                          "font-mono text-[10px]",
                          d.decided_by === "claude" && "text-deck-live",
                          d.decided_by === "human" && "text-deck-attention",
                          d.decided_by === "code" && "text-deck-faint",
                        )}
                      >
                        {d.decided_by}
                        {d.repaired && " · repaired"}
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>

          <section className="flex shrink-0 flex-col gap-3 pt-0.5">
            <SectionRule label="Scheduler" />
            <div className="flex items-center gap-[5px]">
              {Array.from({ length: Math.max(snapshot?.max_concurrent ?? 0, 1) }).map((_, i) => (
                <span
                  key={i}
                  className={cn(
                    "h-[22px] grow rounded-[5px] border",
                    i < (snapshot?.engaged ?? 0)
                      ? "border-deck-live/40 bg-deck-live/[0.22]"
                      : "border-white/[0.08] bg-white/[0.04]",
                  )}
                />
              ))}
            </div>
            <div className="flex items-center justify-between font-mono text-[10.5px] text-deck-dim">
              <span>
                {snapshot?.engaged ?? 0} of {snapshot?.max_concurrent ?? 0} workers engaged
              </span>
              <span>${(snapshot?.spent_usd ?? 0).toFixed(2)} today</span>
            </div>
          </section>
        </aside>
      </div>
    </div>
  );
}

function Counter({
  value,
  label,
  tone,
}: {
  value: number;
  label: string;
  tone?: "done" | "live" | "attention";
}) {
  // Zero is never coloured. A green "0 done" and an amber "0 blocked" both claim a state that has
  // not happened, and the colours only mean something if they appear when the thing is true.
  const lit = value > 0 && tone;
  return (
    <div className="flex flex-col gap-[5px]">
      <span
        className={cn(
          "font-mono text-[26px] leading-[30px] font-medium",
          lit && tone === "done" && "text-deck-done",
          lit && tone === "live" && "text-deck-live",
          lit && tone === "attention" && "text-deck-attention",
          !lit && "text-deck-dim",
        )}
      >
        {value}
      </span>
      <span className="label-micro">{label}</span>
    </div>
  );
}

/** Which loop stage the run is in, inferred from the phase it reports. */
function currentStage(snapshot: RunSnapshot | null): string {
  switch (snapshot?.phase) {
    case "planning":
    case "replanning":
      return "plan";
    case "dispatching":
      return "assign";
    case "reviewing":
      return "review";
    case "blockedonhuman":
      return "escalate";
    default:
      return "observe";
  }
}

function elapsed(since: number | undefined, now: number): string {
  if (!since) return "0m";
  const mins = Math.max(0, Math.floor((now - since) / 60000));
  return mins < 60 ? `${mins}m` : `${Math.floor(mins / 60)}h ${mins % 60}m`;
}

/**
 * The empty state, which is also how every run begins.
 *
 * Given the whole screen rather than a field in a header: with no run there is nothing else to
 * show, and a dashboard of empty panels around one input is a worse first impression than the
 * single thing the operator came here to do.
 */
function StartScreen({
  objective,
  onObjectiveChange,
  onStart,
  busy,
  autonomy,
  onAutonomyChange,
  error,
}: {
  objective: string;
  onObjectiveChange: (v: string) => void;
  onStart: () => void;
  busy: boolean;
  autonomy: Autonomy;
  onAutonomyChange: (m: Autonomy) => void;
  error: string | null;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center px-8">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (objective.trim() && !busy) onStart();
        }}
        className="animate-rise w-full max-w-2xl"
      >
        <div className="label-micro mb-2">New run</div>
        <h1 className="text-[28px] leading-[34px] font-semibold tracking-[-0.025em] text-deck-text">
          What should the team build?
        </h1>
        <p className="mt-2 text-[13px] leading-5 text-deck-dim">
          The supervisor decomposes this into a task graph, assigns it, and verifies the result
          against acceptance criteria before calling anything done.
        </p>

        <Input
          value={objective}
          onChange={(e) => onObjectiveChange(e.target.value)}
          placeholder="Build a production-ready webhook retry system for the payments service"
          className="mt-5 text-[14px]"
          autoFocus
        />

        <div className="mt-4 flex items-center gap-3">
          <AutonomyPicker value={autonomy} onChange={onAutonomyChange} disabled={busy} />
          <div className="grow" />
          {!objective.trim() && (
            <span className="text-[11px] text-deck-faint">
              Describe what the team should build.
            </span>
          )}
          <Button variant="primary" size="lg" type="submit" disabled={busy || !objective.trim()}>
            <Play /> {busy ? "Starting…" : "Start run"}
          </Button>
        </div>

        {error && <div className="mt-3 text-[11px] text-deck-danger">{error}</div>}

        <RunHistory onReuse={onObjectiveChange} />
      </form>
    </div>
  );
}

/**
 * What this project has been asked to do before.
 *
 * A run cannot literally be resumed — its agents exited with the app and its loop is gone — so
 * this offers the objective back rather than pretending otherwise. That is the useful part
 * anyway: picking up after closing the app almost always means running the same thing again,
 * and retyping it from memory loses the wording the plan was built from.
 */
function RunHistory({ onReuse }: { onReuse: (objective: string) => void }) {
  const [runs, setRuns] = useState<PastRunSummary[]>([]);

  useEffect(() => {
    void invoke<PastRunSummary[]>("list_runs")
      .then(setRuns)
      .catch(() => {});
  }, []);

  if (runs.length === 0) return null;

  return (
    <section className="mt-8 flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <History className="size-3.5 text-deck-faint" />
        <span className="label-micro">Earlier runs</span>
      </div>
      <div className="flex flex-col gap-1">
        {runs.slice(0, 5).map((run) => (
          <button
            key={run.run_id}
            type="button"
            onClick={() => onReuse(run.objective)}
            className="card-row group flex items-center gap-3 px-3 py-2 text-left"
          >
            <span
              className={cn(
                "size-1.5 shrink-0 rounded-full",
                run.status === "completed" && "bg-deck-done",
                run.status === "cancelled" && "bg-deck-faint",
                run.status === "failed" && "bg-deck-danger",
                !["completed", "cancelled", "failed"].includes(run.status) &&
                  "bg-deck-attention",
              )}
            />
            <span className="min-w-0 grow truncate text-[12.5px] text-deck-dim">
              {run.objective}
            </span>
            <span className="shrink-0 font-mono text-[10px] text-deck-faint">
              {run.status} · {run.task_count} tasks · ${run.spent_usd.toFixed(2)}
            </span>
            <RotateCcw className="size-3 shrink-0 text-deck-faint opacity-0 transition-opacity group-hover:opacity-100" />
          </button>
        ))}
      </div>
    </section>
  );
}
