import { invoke } from "@tauri-apps/api/core";
import { Boxes, PanelsTopLeft } from "lucide-react";
import { useEffect, useState } from "react";
import { TooltipProvider } from "./components/ui/tooltip";
import { cn } from "./lib/utils";
import { EscalationLayer } from "./features/permissions/EscalationLayer";
import { RecoveryBanner } from "./features/recovery/RecoveryBanner";
import { RuntimeGate } from "./features/setup/RuntimeGate";
import { TeamView } from "./features/team/TeamView";
import { TranscriptView } from "./features/sessions/TranscriptView";
import {
  useEventPump,
  usePumpStats,
  useSessionSubscriptions,
} from "./hooks/useEventPump";
import "./index.css";

/**
 * M1 shell.
 *
 * This is intentionally *not* a chat window with a sidebar. The spec's central point is that
 * the product is Objective → Supervisor Run → Task Graph → Agents → Sessions, and that tabs
 * are only one way to observe sessions. So the layout is a dashboard frame with the session
 * transcript occupying a panel inside it — the Team View content fills in at M4/M6, but the
 * hierarchy is established now rather than retrofitted around a chat.
 */
export default function App() {
  return (
    // Everything below assumes a working `claude` CLI, so nothing below mounts until there is
    // one — including the event pump, which would otherwise stream an empty transcript at
    // someone whose real problem is that the CLI is not installed.
    // Radix tooltips need one provider above everything that uses them; a short delay keeps
    // them from firing while the pointer is merely crossing the panel.
    <TooltipProvider delayDuration={350} skipDelayDuration={0}>
      <RuntimeGate>
        <Deck />
      </RuntimeGate>
    </TooltipProvider>
  );
}

function Deck() {
  useEventPump();

  const [fixtures, setFixtures] = useState<string[]>([]);
  const [sessions, setSessions] = useState<string[]>([]);
  const [active, setActive] = useState<string | null>(null);
  // Team View is the landing surface, not a chat. Sessions are reachable from it rather than the
  // other way round, which is what keeps the product from becoming a tab manager.
  const [view, setView] = useState<"team" | "sessions">("team");
  const stats = usePumpStats();

  // Rust drops token deltas for anything not in this list, so it must reflect what is visible.
  useSessionSubscriptions(active ? [active] : []);

  useEffect(() => {
    void invoke<string[]>("list_fixtures").then(setFixtures).catch(() => {});
  }, []);

  async function startReplay(name: string) {
    const id = await invoke<string>("replay_fixture", { name });
    setSessions((prev) => [...prev, id]);
    setActive(id);
  }

  return (
    <div className="relative flex h-full flex-col text-deck-text">
      <header className="glass-flat flex h-10 shrink-0 items-center justify-between border-b border-white/8 px-3">
        <div className="flex items-center gap-3">
          <span className="flex items-center gap-1.5 text-[13px] font-semibold tracking-tight">
            <Boxes className="size-4 text-deck-live" />
            AgentDeck
          </span>
          <nav className="flex gap-0.5 rounded-md border border-white/8 bg-black/20 p-0.5">
            {TABS.map((tab) => (
              <button
                key={tab.id}
                onClick={() => setView(tab.id)}
                className={cn(
                  "flex items-center gap-1.5 rounded px-2 py-0.5 text-[11px] transition-colors",
                  view === tab.id
                    ? "bg-white/12 font-medium text-deck-text"
                    : "text-deck-faint hover:text-deck-dim",
                )}
              >
                {tab.icon}
                {tab.label}
              </button>
            ))}
          </nav>
        </div>
        {/* Pump telemetry. Kept because a growing gap count is the first sign the event bridge
            is dropping batches, and that is invisible everywhere else. */}
        <div className="flex items-center gap-3 font-mono text-[10px] text-deck-faint">
          <span>{stats.batches} batches</span>
          <span>{stats.events} events</span>
          <span className={stats.gaps > 0 ? "text-deck-attention" : undefined}>
            {stats.gaps} gaps
          </span>
        </div>
      </header>

      {/* Outside the router and the session panel: an agent can block while the operator is
          looking elsewhere, and a prompt buried in a hidden transcript would time out unseen. */}
      <EscalationLayer />

      {/* Above the view switch: what a crash left behind is true of the whole app, not of
          whichever surface happens to be open. */}
      <RecoveryBanner />

      {view === "team" ? (
        <div className="min-h-0 flex-1">
          <TeamView onOpenSession={() => setView("sessions")} />
        </div>
      ) : (
      <div className="flex min-h-0 flex-1">
        <aside className="glass-flat flex w-56 shrink-0 flex-col border-r border-white/8">
          <Section title="Replay a captured session">
            {fixtures.length === 0 && (
              <p className="px-2 text-[11px] text-deck-faint">No fixtures embedded</p>
            )}
            {fixtures.map((name) => (
              <button
                key={name}
                onClick={() => void startReplay(name)}
                className="w-full rounded px-2 py-1 text-left text-[12px] text-deck-dim transition-colors hover:bg-white/8 hover:text-deck-text"
              >
                {name}
              </button>
            ))}
          </Section>

          <Section title={`Sessions (${sessions.length})`}>
            {sessions.length === 0 && (
              <p className="px-2 text-[11px] leading-relaxed text-deck-faint">
                Start a replay to stream a captured session — no CLI or rate limit needed.
              </p>
            )}
            {sessions.map((id, i) => (
              <button
                key={id}
                onClick={() => setActive(id)}
                className={cn(
                  "flex w-full items-center gap-2 rounded px-2 py-1 text-left text-[12px] transition-colors",
                  active === id
                    ? "bg-white/12 text-deck-text"
                    : "text-deck-dim hover:bg-white/6",
                )}
              >
                <span
                  className={cn(
                    "size-1.5 shrink-0 rounded-full",
                    active === id ? "ring-live animate-live bg-deck-live" : "bg-deck-faint",
                  )}
                />
                <span className="truncate font-mono text-[11px]">
                  session {i + 1} · {id.slice(0, 8)}
                </span>
              </button>
            ))}
          </Section>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col">
          <div className="glass-flat flex h-8 shrink-0 items-center border-b border-white/8 px-3 text-[11px] text-deck-faint">
            {active ? (
              <span className="font-mono">{active}</span>
            ) : (
              <span>No active session</span>
            )}
          </div>
          <div className="min-h-0 flex-1">
            <TranscriptView sessionId={active} />
          </div>
        </main>
      </div>
      )}

      <footer className="flex h-6 shrink-0 items-center gap-3 border-t border-white/6 px-3 font-mono text-[10px] text-deck-faint">
        <span>Sessions: {sessions.length}</span>
        <span>Watching: {active ? 1 : 0}</span>
        <span>Last seq: {stats.lastSeq}</span>
      </footer>
    </div>
  );
}

const TABS = [
  { id: "team" as const, label: "Team", icon: <Boxes className="size-3" /> },
  { id: "sessions" as const, label: "Sessions", icon: <PanelsTopLeft className="size-3" /> },
];

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="border-b border-white/6 p-2">
      <h2 className="label-micro mb-1.5 px-2">{title}</h2>
      <div className="space-y-0.5">{children}</div>
    </div>
  );
}
