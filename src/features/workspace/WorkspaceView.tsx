import { invoke } from "@tauri-apps/api/core";
import { GitBranch, Square, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";
import type { AgentSummary, EscalationAnswer, RunSnapshot } from "../../lib/types";
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
  onOpenTeam,
}: {
  sessions: string[];
  active: string | null;
  onSelect: (sessionId: string) => void;
  onClose: (sessionId: string) => void;
  onOpenTeam: () => void;
}) {
  const [snapshot, setSnapshot] = useState<RunSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [answering, setAnswering] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await invoke<RunSnapshot>("get_run_snapshot"));
    } catch {
      // A failed poll is not worth surfacing; the next one will succeed.
    }
  }, []);

  useEffect(() => {
    void refresh();
    const id = setInterval(() => void refresh(), 1000);
    return () => clearInterval(id);
  }, [refresh]);

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

  /**
   * Replays a captured session.
   *
   * Kept reachable because it is the only way to exercise the transcript without an
   * authenticated CLI or spending the account's rate limit — the entrance for anyone working on
   * this screen, and the demo path when there is no run.
   */
  async function replay(name: string) {
    try {
      const id = await invoke<string>("replay_fixture", { name });
      onSelect(id);
    } catch (e) {
      setError(String(e));
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
  const escalations = snapshot?.escalations ?? [];

  return (
    <div className="flex min-h-0 grow">
      <WorkspaceSidebar
        snapshot={snapshot}
        activeSession={active}
        onOpenSession={onSelect}
      />

      <main className="flex min-w-0 grow flex-col">
        <SessionTabs
          sessions={sessions}
          active={active}
          agentFor={agentFor}
          onSelect={onSelect}
          onClose={onClose}
          onOpenTeam={onOpenTeam}
        />

        {current && <SessionHeader agent={current} onKill={kill} />}

        {error && (
          <div className="shrink-0 border-b border-deck-attention/25 bg-deck-attention/[0.08] px-4 py-1.5 text-[11px] text-deck-attention">
            {error}
          </div>
        )}

        <div className="min-h-0 grow">
          {active ? <TranscriptView sessionId={active} /> : <NoSession onReplay={replay} />}
        </div>
      </main>

      <aside className="glass-flat flex w-[300px] shrink-0 flex-col gap-5 overflow-y-auto border-l border-white/[0.07] p-4">
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
      </aside>
    </div>
  );
}

function NoSession({ onReplay }: { onReplay: (name: string) => void }) {
  const [fixtures, setFixtures] = useState<string[]>([]);

  useEffect(() => {
    void invoke<string[]>("list_fixtures")
      .then(setFixtures)
      .catch(() => {});
  }, []);

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 px-8">
      <div className="max-w-md text-center">
        <p className="text-[13px] text-deck-dim">No session open.</p>
        <p className="mt-1 text-[11.5px] leading-relaxed text-deck-faint">
          Pick an agent from the roster to read what it is doing, or replay a captured session —
          no CLI and no rate limit needed.
        </p>
      </div>
      {fixtures.length > 0 && (
        <div className="flex flex-wrap justify-center gap-1.5">
          {fixtures.map((name) => (
            <Button key={name} variant="secondary" size="sm" onClick={() => onReplay(name)}>
              {name}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}

function SessionTabs({
  sessions,
  active,
  agentFor,
  onSelect,
  onClose,
  onOpenTeam,
}: {
  sessions: string[];
  active: string | null;
  agentFor: (id: string) => AgentSummary | null;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onOpenTeam: () => void;
}) {
  return (
    <div className="glass-flat flex h-10 shrink-0 items-end gap-0.5 border-b border-white/[0.07] px-2.5">
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
            className={cn(
              "group flex h-[31px] items-center gap-2 rounded-t-lg px-3 transition-colors",
              isActive
                ? "border-t border-r border-l border-white/[0.09] bg-white/[0.06]"
                : "hover:bg-white/[0.03]",
            )}
          >
            <button onClick={() => onSelect(id)} className="flex items-center gap-2">
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
                {agent?.name ?? "Session"}
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
      <button
        onClick={onOpenTeam}
        className="pb-2.5 text-[11px] text-deck-faint transition-colors hover:text-deck-dim"
      >
        ← Team
      </button>
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
    <header className="flex h-[52px] shrink-0 items-center gap-3 border-b border-white/[0.07] px-4">
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
