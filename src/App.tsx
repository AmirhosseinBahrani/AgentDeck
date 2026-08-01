import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { TooltipProvider } from "./components/ui/tooltip";
import { EscalationLayer } from "./features/permissions/EscalationLayer";
import { RecoveryBanner } from "./features/recovery/RecoveryBanner";
import { ProjectGate, useProjectPicker } from "./features/setup/ProjectGate";
import { RuntimeGate } from "./features/setup/RuntimeGate";
import { NavTabs, type NavTab } from "./features/shell/NavTabs";
import { TitleBar } from "./features/shell/TitleBar";
import { TeamView } from "./features/team/TeamView";
import { DecisionsView } from "./features/views/DecisionsView";
import { DiffsView } from "./features/views/DiffsView";
import { SupervisorView } from "./features/views/SupervisorView";
import { TaskGraphView } from "./features/views/TaskGraphView";
import { AdvancedView } from "./features/views/AdvancedView";
import { MemoryView } from "./features/views/MemoryView";
import { SkillsView } from "./features/views/SkillsView";
import { WorkspaceView } from "./features/workspace/WorkspaceView";
import { useEventPump, usePumpStats, useSessionSubscriptions } from "./hooks/useEventPump";
import type { ProjectInfo, RunSnapshot } from "./lib/types";
import "./index.css";

/**
 * The shell.
 *
 * Deliberately *not* a chat window with a sidebar. The product is Objective → Run → Task Graph →
 * Agents → Sessions, and a transcript is one way to observe a session rather than the thing
 * itself — so Team is the landing surface and the workspace is reached from it, never the other
 * way round. That ordering is what stops this becoming a tab manager.
 */
export default function App() {
  return (
    // Nothing mounts until there is a working CLI — including the event pump, which would
    // otherwise stream an empty transcript at someone whose real problem is that nothing is
    // installed. Radix tooltips need one provider above everything that uses them; the delay
    // keeps them from firing while the pointer is merely crossing a panel.
    <TooltipProvider delayDuration={350} skipDelayDuration={0}>
      <RuntimeGate>
        <ProjectGate>
          <Deck />
        </ProjectGate>
      </RuntimeGate>
    </TooltipProvider>
  );
}

function Deck() {
  useEventPump();

  const [snapshot, setSnapshot] = useState<RunSnapshot | null>(null);
  const [project, setProject] = useState<ProjectInfo | null>(null);
  const [sessions, setSessions] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [view, setView] = useState<NavTab>("team");
  const stats = usePumpStats();
  const picker = useProjectPicker(setProject);

  // Rust drops token deltas for anything not in this list, so it must reflect what is visible.
  useSessionSubscriptions(active ? [active] : []);

  // Polled here as well as inside the views: the title bar sits outside both and still has to
  // show the autonomy mode the supervisor is actually enforcing.
  useEffect(() => {
    const read = () =>
      void invoke<RunSnapshot>("get_run_snapshot")
        .then(setSnapshot)
        .catch(() => {});
    read();
    const id = setInterval(read, 2000);
    void invoke<ProjectInfo>("get_project").then(setProject).catch(() => {});
    return () => clearInterval(id);
  }, []);

  /** Opens a session, registering it first so a live agent is reachable rather than unknown. */
  function openSession(sessionId?: string) {
    if (sessionId) {
      setSessions((prev) => (prev.includes(sessionId) ? prev : [...prev, sessionId]));
      setActive(sessionId);
    }
    setView("workspace");
  }

  return (
    <div className="relative flex h-full flex-col text-deck-text">
      {picker.dialog}

      <TitleBar
        project={project?.name ?? "no project"}
        projectPath={project?.path ?? undefined}
        onChangeProject={() => void picker.pick()}
        autonomy={snapshot?.autonomy || "assisted"}
        running={!!snapshot?.active}
        phase={snapshot?.phase ?? ""}
        startedAt={snapshot?.started_at_ms ?? 0}
      />

      {/* Outside both views: an agent can block while the operator is looking elsewhere, and a
          prompt buried in a hidden transcript would time out unseen. */}
      <EscalationLayer />

      {/* Above the view switch, because what a crash left behind is true of the whole app. */}
      <RecoveryBanner />

      <NavTabs
        active={view}
        onChange={setView}
        trailing={
          snapshot?.run_id ? (
            <span className="font-mono text-[10.5px] text-deck-faint">
              run {snapshot.run_id}
            </span>
          ) : undefined
        }
      />

      {view === "team" && (
        <div className="min-h-0 flex-1">
          <TeamView
            onOpenSession={openSession}
            projectPath={project?.path ?? null}
            onProjectChanged={() =>
              void invoke<ProjectInfo>("get_project")
                .then(setProject)
                .catch(() => {})
            }
          />
        </div>
      )}

      {view === "graph" && (
        <div className="min-h-0 flex-1">
          <TaskGraphView snapshot={snapshot} onOpenSession={openSession} />
        </div>
      )}

      {view === "diffs" && (
        <div className="min-h-0 flex-1">
          <DiffsView />
        </div>
      )}

      {view === "decisions" && (
        <div className="min-h-0 flex-1">
          <DecisionsView snapshot={snapshot} />
        </div>
      )}

      {view === "supervisor" && (
        <div className="min-h-0 flex-1">
          <SupervisorView snapshot={snapshot} />
        </div>
      )}

      {view === "memory" && (
        <div className="flex min-h-0 flex-1 flex-col">
          <MemoryView project={project?.name ?? null} />
        </div>
      )}

      {view === "skills" && (
        <div className="flex min-h-0 flex-1 flex-col">
          <SkillsView project={project?.name ?? null} />
        </div>
      )}

      {view === "advanced" && (
        <div className="flex min-h-0 flex-1 flex-col">
          <AdvancedView project={project?.name ?? null} />
        </div>
      )}

      {view === "workspace" && (
        <WorkspaceView
          sessions={sessions}
          active={active}
          // `openSession` rather than `setActive`: the sidebar can name a session that has no tab
          // yet — an agent from the roster, or one from an earlier run — and selecting it without
          // registering it left the transcript showing with nothing in the tab strip to close.
          onSelect={openSession}
          onClose={(id) => {
            setSessions((prev) => prev.filter((s) => s !== id));
            // Focus falls to whatever is left rather than to nothing: closing a tab and landing
            // on an empty pane loses your place for no reason.
            setActive((cur) => (cur === id ? (sessions.find((s) => s !== id) ?? null) : cur));
          }}
        />
      )}

      <footer className="flex h-6 shrink-0 items-center gap-4 border-t border-white/[0.06] px-3 font-mono text-[10px] text-deck-faint">
        <span>{snapshot?.engaged ?? 0} running</span>
        <span>{snapshot?.agents.length ?? 0} agents</span>
        <span>{snapshot?.tasks.length ?? 0} tasks</span>
        <span>{sessions.length} sessions</span>
        <div className="grow" />
        <span>{stats.events} events</span>
        {/* A growing gap count is the first sign the event bridge is dropping batches, and that
            is invisible everywhere else in the app. */}
        <span className={stats.gaps > 0 ? "text-deck-attention" : undefined}>
          {stats.gaps} gaps
        </span>
        <span>${(snapshot?.spent_usd ?? 0).toFixed(2)} today</span>
      </footer>
    </div>
  );
}

