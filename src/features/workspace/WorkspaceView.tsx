import { invoke } from "@tauri-apps/api/core";
import { GitBranch, Square, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";
import type {
  AgentSummary,
  EscalationAnswer,
  RunSnapshot,
  SessionHistoryEntry,
  TaskSummary,
} from "../../lib/types";
import { cn } from "../../lib/utils";
import { EscalationInbox } from "../team/EscalationInbox";
import { TranscriptView } from "../sessions/TranscriptView";
import { WorkspaceSidebar } from "./WorkspaceSidebar";

/**
 * The working surface: one agent's transcript, framed by everything you need to judge it.
 *
 * The three columns answer three different questions and none of them is the transcript alone —
 * who else is working (left), what is this agent doing (centre), and what is the run as a whole
 * waiting on (right). A bare transcript would be a terminal; the frame is what makes it
 * supervision.
 */
export function WorkspaceView({
  sessions,
  active,
  onSelect,
  onClose,
  onReorder,
}: {
  sessions: string[];
  active: string | null;
  onSelect: (sessionId: string) => void;
  onClose: (sessionId: string) => void;
  onReorder: (next: string[]) => void;
}) {
  const [snapshot, setSnapshot] = useState<RunSnapshot | null>(null);
  const [history, setHistory] = useState<SessionHistoryEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [answering, setAnswering] = useState<string | null>(null);
  const [openTask, setOpenTask] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await invoke<RunSnapshot>("get_run_snapshot"));
    } catch {
      // A failed poll is not worth surfacing; the next one will succeed.
    }
  }, []);

  // Polled far more slowly than the snapshot. History only changes when a session ends, and
  // joining three tables every second to learn nothing has changed is not worth it.
  useEffect(() => {
    const load = () =>
      void invoke<SessionHistoryEntry[]>("list_session_history")
        .then(setHistory)
        .catch(() => {});
    load();
    const id = setInterval(load, 15_000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    void refresh();
    const id = setInterval(() => void refresh(), 1000);
    return () => clearInterval(id);
  }, [refresh]);

  /** Removes finished session records. The list is the only place their absence is felt. */
  async function clearHistory() {
    try {
      setError(await invoke<string>("clear_session_history"));
      setHistory(await invoke<SessionHistoryEntry[]>("list_session_history"));
    } catch (e) {
      setError(String(e));
    }
  }

  async function answer(escalationId: string, choice: EscalationAnswer) {
    setAnswering(escalationId);
    try {
      await invoke("answer_escalation", { escalationId, answer: choice });
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setAnswering(null);
    }
  }

  async function kill(taskId: string) {
    try {
      await invoke<boolean>("force_kill_agent", { taskId });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  const agentFor = (sessionId: string | null) =>
    snapshot?.agents.find((a) => a.session_id === sessionId) ?? null;
  const current = agentFor(active);
  // Falls back to history so a transcript from an earlier run is labelled with who wrote it
  // rather than the word "Session".
  const nameFor = (id: string) =>
    agentFor(id)?.name ?? history.find((h) => h.session_id === id)?.agent_name ?? "Session";
  const pastEntry = active && !current ? history.find((h) => h.session_id === active) : undefined;
  const escalations = snapshot?.escalations ?? [];
  // Resolved against the live snapshot rather than stored, so a selected task that disappears
  // between polls closes the panel instead of rendering a stale copy of itself.
  const selectedTask = snapshot?.tasks.find((t) => t.id === openTask) ?? null;

  return (
    <div className="flex min-h-0 grow">
      <WorkspaceSidebar
        snapshot={snapshot}
        history={history}
        activeSession={active}
        activeTask={openTask}
        onOpenSession={(id) => {
          setOpenTask(null);
          onSelect(id);
        }}
        onOpenTask={setOpenTask}
        onClearHistory={clearHistory}
      />

      <main className="flex min-w-0 grow flex-col">
        <SessionTabs
          sessions={sessions}
          active={active}
          agentFor={agentFor}
          nameFor={nameFor}
          onSelect={onSelect}
          onClose={onClose}
          onReorder={onReorder}
        />

        {current && <SessionHeader agent={current} onKill={kill} />}

        {pastEntry && (
          <div className="flex shrink-0 items-center gap-2 border-b border-deck-line px-4 py-1.5 text-[11px] text-deck-faint">
            <span className="text-deck-dim">{pastEntry.agent_name}</span>
            <span>·</span>
            <span>{pastEntry.task_title ?? "no task"}</span>
            <span className="ml-auto font-mono">
              {pastEntry.status} · ${pastEntry.cost_usd.toFixed(2)}
            </span>
          </div>
        )}

        {error && (
          <div className="shrink-0 border-b border-deck-attention/25 bg-deck-attention-wash px-4 py-1.5 text-[11px] text-deck-attention">
            {error}
          </div>
        )}

        <div className="min-h-0 grow">
          {active ? (
            <TranscriptView sessionId={active} />
          ) : (
            <NoSession history={history} onOpen={onSelect} />
          )}
        </div>
      </main>

      <aside className="glass-flat flex w-[300px] shrink-0 flex-col gap-5 overflow-y-auto border-l border-deck-line p-4">
        {selectedTask ? (
          <TaskDetail
            task={selectedTask}
            agent={snapshot?.agents.find((a) => a.task_id === selectedTask.id) ?? null}
            onClose={() => setOpenTask(null)}
            onOpenSession={onSelect}
          />
        ) : (
          <>
        <section className="flex flex-col gap-2">
          <SectionRule label="Objective" trailing={snapshot?.run_id ? `run ${snapshot.run_id}` : undefined} />
          {snapshot?.objective ? (
            <>
              <p className="text-[13px] leading-5 text-deck-text">{snapshot.objective}</p>
              <div className="flex items-center gap-2">
                <Badge tone={snapshot.phase === "blockedonhuman" ? "attention" : "live"}>
                  iteration {snapshot.iteration} · {snapshot.phase}
                </Badge>
              </div>
            </>
          ) : (
            <p className="text-[11px] leading-relaxed text-deck-faint">
              No run. Start one from the Team view.
            </p>
          )}
        </section>

        {escalations.length > 0 && (
          <section className="flex flex-col gap-2">
            <SectionRule label="Needs your decision" accent />
            <EscalationInbox escalations={escalations} onAnswer={answer} busy={answering} />
          </section>
        )}

        <section className="flex min-h-0 grow flex-col gap-2">
          <SectionRule label="Decision log" />
          {!snapshot?.decisions.length ? (
            <p className="text-[11px] leading-relaxed text-deck-faint">
              Nothing decided yet.
            </p>
          ) : (
            <div className="flex flex-col gap-2.5">
              {snapshot.decisions.slice(0, 15).map((d, i) => (
                <div key={i} className="flex gap-2.5">
                  <span className="w-6 shrink-0 pt-px font-mono text-[10px] text-deck-faint">
                    #{d.iteration}
                  </span>
                  <div className="flex min-w-0 grow flex-col gap-0.5">
                    <span
                      className={cn(
                        "text-[12px] leading-4",
                        i === 0 ? "text-deck-text" : "text-deck-dim",
                      )}
                    >
                      {d.kind.replace(/_/g, " ")}
                    </span>
                    {d.rationale && (
                      <span className="text-[11px] leading-4 text-deck-faint">{d.rationale}</span>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>
          </>
        )}
      </aside>
    </div>
  );
}

/**
 * One task, in full.
 *
 * Takes over the right column rather than opening over the transcript: the two are meant to be
 * read together — the contract on one side, what the agent actually did on the other — and a
 * dialog would cover exactly the thing it is describing.
 */
function TaskDetail({
  task,
  agent,
  onClose,
  onOpenSession,
}: {
  task: TaskSummary;
  agent: AgentSummary | null;
  onClose: () => void;
  onOpenSession: (sessionId: string) => void;
}) {
  const facts: [string, string][] = [
    ["Status", task.status],
    ["Role", task.role],
    ["Attempts", String(task.attempts)],
    ["Review rounds", String(task.review_rounds)],
    ["Objective gate", task.objective_gate ? "yes" : "no"],
    ["Agent", agent?.name ?? "unassigned"],
  ];
  if (agent?.branch) facts.push(["Branch", agent.branch]);

  return (
    <>
      <section className="flex flex-col gap-2">
        <div className="flex items-start justify-between gap-2">
          <SectionRule label="Task" />
          <Button variant="ghost" size="sm" onClick={onClose} className="-mt-1 -mr-1">
            <X />
          </Button>
        </div>
        <p className="text-[13px] leading-5 text-deck-text">{task.title}</p>
        <span className="font-mono text-[10px] text-deck-faint">{task.id}</span>
      </section>

      {task.awaiting_approval && (
        <p className="rounded-md border border-deck-attention/30 bg-deck-attention-wash px-2.5 py-2 text-[11.5px] leading-relaxed text-deck-attention">
          Assigned and ready, but this autonomy mode needs you to start it.
        </p>
      )}

      {task.blocked_reason && (
        <section className="flex flex-col gap-1.5">
          <SectionRule label="Blocked" accent />
          <p className="text-[11.5px] leading-relaxed text-deck-attention">
            {task.blocked_reason}
          </p>
        </section>
      )}

      <section className="flex flex-col gap-1.5">
        <SectionRule label="Detail" />
        {facts.map(([label, value]) => (
          <div key={label} className="flex items-baseline justify-between gap-3">
            <span className="shrink-0 text-[11.5px] text-deck-faint">{label}</span>
            <span className="min-w-0 truncate text-right font-mono text-[11px] text-deck-dim">
              {value}
            </span>
          </div>
        ))}
      </section>

      {task.session_id && (
        <Button variant="secondary" size="sm" onClick={() => onOpenSession(task.session_id!)}>
          Open this agent's transcript
        </Button>
      )}
    </>
  );
}

/**
 * What to offer when nothing is open.
 *
 * This used to list the recorded protocol fixtures. They exist to develop the transcript renderer
 * without an authenticated CLI, which is a real need — but it is a need of whoever is working on
 * this screen, not of whoever is running a team, and putting four internal test names in front of
 * every operator implied they were something to use. Recent sessions are what the space is
 * actually for: picking up where you left off is the ordinary reason to be here with nothing open.
 */
function NoSession({
  history,
  onOpen,
}: {
  history: SessionHistoryEntry[];
  onOpen: (sessionId: string) => void;
}) {
  const recent = history.slice(0, 6);

  return (
    <div className="flex h-full flex-col items-center justify-center gap-5 px-8">
      <div className="max-w-md text-center">
        <p className="text-[13px] text-deck-dim">No session open.</p>
        <p className="mt-1 text-[11.5px] leading-relaxed text-deck-faint">
          {recent.length > 0
            ? "Pick an agent from the roster to watch it work, or reopen one of these."
            : "Pick an agent from the roster to read what it is doing. Sessions you have run appear here once there are some."}
        </p>
      </div>

      {recent.length > 0 && (
        <div className="flex w-full max-w-md flex-col gap-1">
          {recent.map((entry) => (
            <button
              key={entry.session_id}
              onClick={() => onOpen(entry.session_id)}
              className="flex items-center gap-3 rounded-md border border-deck-line bg-deck-surface px-3 py-2 text-left transition-colors hover:bg-deck-raised"
            >
              <span className="shrink-0 text-[12.5px] font-medium text-deck-text">
                {entry.agent_name}
              </span>
              <span className="min-w-0 grow truncate text-[11.5px] text-deck-faint">
                {entry.task_title ?? "no task"}
              </span>
              <span className="shrink-0 font-mono text-[10.5px] text-deck-faint">
                {entry.status}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The open transcripts, in an order the operator controls.
 *
 * Tabs arrive in the order sessions happened to be opened, which is rarely the order anyone
 * wants to read them in — the two agents you are comparing end up at opposite ends of the strip
 * with unrelated ones between. Dragging is the cheapest fix: no menu, no settings, and the
 * result is visible where the change was made.
 */
function SessionTabs({
  sessions,
  active,
  agentFor,
  nameFor,
  onSelect,
  onClose,
  onReorder,
}: {
  sessions: string[];
  active: string | null;
  agentFor: (id: string) => AgentSummary | null;
  nameFor: (id: string) => string;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onReorder: (next: string[]) => void;
}) {
  const [dragging, setDragging] = useState<string | null>(null);
  const [over, setOver] = useState<string | null>(null);

  function drop(target: string) {
    if (!dragging || dragging === target) {
      setDragging(null);
      setOver(null);
      return;
    }
    const next = sessions.filter((id) => id !== dragging);
    next.splice(next.indexOf(target), 0, dragging);
    onReorder(next);
    setDragging(null);
    setOver(null);
  }

  return (
    <div className="glass-flat flex h-10 shrink-0 items-end gap-0.5 border-b border-deck-line px-2.5">
      {sessions.length === 0 && (
        <span className="pb-2.5 pl-1 text-[11px] text-deck-faint">
          No sessions open. Pick an agent from the roster to read what it is doing.
        </span>
      )}
      {sessions.map((id) => {
        const agent = agentFor(id);
        const isActive = id === active;
        return (
          <div
            key={id}
            draggable
            onDragStart={(e) => {
              setDragging(id);
              e.dataTransfer.effectAllowed = "move";
            }}
            onDragEnd={() => {
              setDragging(null);
              setOver(null);
            }}
            // Without preventDefault the browser refuses the drop and the tab springs back.
            onDragOver={(e) => {
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              if (id !== dragging) setOver(id);
            }}
            onDragLeave={() => setOver((cur) => (cur === id ? null : cur))}
            onDrop={(e) => {
              e.preventDefault();
              drop(id);
            }}
            className={cn(
              "group flex h-[31px] cursor-grab items-center gap-2 rounded-t-lg pr-3 pl-2.5 transition-colors active:cursor-grabbing",
              // A line on the edge the tab would land against, rather than moving the other tabs
              // out of the way. Reflowing the strip under the cursor makes the target you were
              // aiming at the one thing that moves.
              over === id && "shadow-[inset_2px_0_0_0_var(--deck-accent)]",
              dragging === id && "opacity-40",
              // The focused tab is lifted to the page ground rather than tinted accent — a strip
              // of them would otherwise compete with the selected row in the sidebar.
              isActive
                ? "border-t border-r border-l border-deck-line bg-deck-bg"
                : "hover:bg-deck-raised",
            )}
          >
            {/* A grip that only appears on hover. The strip is dense and a permanent handle on
                every tab would read as clutter, but without any affordance nothing suggests the
                tabs can be moved at all. */}
            <span className="-ml-1.5 flex w-2 shrink-0 justify-center opacity-0 transition-opacity group-hover:opacity-100">
              <svg width="6" height="12" viewBox="0 0 6 12" fill="none">
                <circle cx="1.5" cy="2.5" r="0.9" fill="var(--deck-faint)" />
                <circle cx="4.5" cy="2.5" r="0.9" fill="var(--deck-faint)" />
                <circle cx="1.5" cy="6" r="0.9" fill="var(--deck-faint)" />
                <circle cx="4.5" cy="6" r="0.9" fill="var(--deck-faint)" />
                <circle cx="1.5" cy="9.5" r="0.9" fill="var(--deck-faint)" />
                <circle cx="4.5" cy="9.5" r="0.9" fill="var(--deck-faint)" />
              </svg>
            </span>
            <button
              draggable={false}
              onClick={() => onSelect(id)}
              className="flex items-center gap-2"
            >
              <span
                className={cn(
                  "size-1.5 shrink-0 rounded-full",
                  agent?.status === "running" && "animate-live bg-deck-live",
                  agent?.status === "blocked" && "bg-deck-attention",
                  !agent && "bg-deck-faint",
                )}
              />
              <span
                className={cn(
                  "text-[12px]",
                  isActive ? "font-semibold text-deck-text" : "text-deck-dim",
                )}
              >
                {nameFor(id)}
              </span>
              <span className="font-mono text-[10px] text-deck-faint">#{id.slice(0, 4)}</span>
            </button>
            {/* Only on the focused tab: a close control on every tab turns a row of them into a
                row of targets, and closing the wrong transcript mid-run is a real cost. */}
            {isActive && (
              <button
                onClick={() => onClose(id)}
                className="text-deck-faint transition-colors hover:text-deck-text"
              >
                <X className="size-3" />
              </button>
            )}
          </div>
        );
      })}
      <div className="grow" />
    </div>
  );
}

function SessionHeader({
  agent,
  onKill,
}: {
  agent: AgentSummary;
  onKill: (taskId: string) => void;
}) {
  const [confirming, setConfirming] = useState(false);

  return (
    <header className="flex h-[52px] shrink-0 items-center gap-3 border-b border-deck-line px-4">
      <div className="flex min-w-0 flex-col gap-0.5">
        <div className="flex items-center gap-2">
          <span className="text-[14px] font-semibold text-deck-text">{agent.name}</span>
          <Badge
            tone={
              agent.status === "running"
                ? "live"
                : agent.status === "blocked"
                  ? "attention"
                  : "neutral"
            }
          >
            {agent.status}
          </Badge>
        </div>
        <span className="flex items-center gap-2 truncate font-mono text-[10.5px] text-deck-faint">
          {agent.activity ?? "no task"}
          {agent.branch && (
            <>
              <GitBranch className="size-3 shrink-0" />
              {agent.branch}
            </>
          )}
        </span>
      </div>

      <div className="grow" />

      {/* Force-kill lives beside the transcript because this is where you decide an agent has
          gone wrong. Behind a confirm: the kill is immediate once clicked, and the turn in
          progress does not survive it — though the worktree and its changes do. */}
      {agent.status === "running" &&
        agent.task_id &&
        (confirming ? (
          <div className="flex items-center gap-1.5">
            <Button
              variant="danger"
              size="sm"
              onClick={() => {
                setConfirming(false);
                onKill(agent.task_id!);
              }}
            >
              Kill now
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
              Cancel
            </Button>
          </div>
        ) : (
          <Button
            variant="secondary"
            size="sm"
            onClick={() => setConfirming(true)}
            title="Stops this agent immediately. Its worktree and changes are kept, and the task is cancelled rather than failed."
          >
            <Square /> Interrupt
          </Button>
        ))}
    </header>
  );
}
