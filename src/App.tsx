import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { EscalationLayer } from "./features/permissions/EscalationLayer";
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
    <div className="relative flex h-full flex-col bg-neutral-950 text-neutral-200">
      <header className="flex h-9 shrink-0 items-center justify-between border-b border-neutral-800 px-3">
        <div className="flex items-center gap-2">
          <span className="text-[13px] font-semibold">AgentDeck</span>
          <nav className="ml-2 flex gap-0.5">
            {(["team", "sessions"] as const).map((tab) => (
              <button
                key={tab}
                onClick={() => setView(tab)}
                className={`rounded px-2 py-0.5 text-[11px] capitalize ${
                  view === tab
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-500 hover:text-neutral-300"
                }`}
              >
                {tab}
              </button>
            ))}
          </nav>
        </div>
        <div className="flex items-center gap-3 text-[11px] text-neutral-500">
          <span>{stats.batches} batches</span>
          <span>{stats.events} events</span>
          <span className={stats.gaps > 0 ? "text-amber-400" : undefined}>
            {stats.gaps} gaps
          </span>
        </div>
      </header>

      {/* Outside the router and the session panel: an agent can block while the operator is
          looking elsewhere, and a prompt buried in a hidden transcript would time out unseen. */}
      <EscalationLayer />

      {view === "team" ? (
        <div className="min-h-0 flex-1">
          <TeamView onOpenSession={() => setView("sessions")} />
        </div>
      ) : (
      <div className="flex min-h-0 flex-1">
        <aside className="flex w-56 shrink-0 flex-col border-r border-neutral-800">
          <Section title="Replay a captured session">
            {fixtures.length === 0 && (
              <p className="px-2 text-[11px] text-neutral-600">No fixtures embedded</p>
            )}
            {fixtures.map((name) => (
              <button
                key={name}
                onClick={() => void startReplay(name)}
                className="w-full rounded px-2 py-1 text-left text-[12px] text-neutral-300 hover:bg-neutral-800"
              >
                {name}
              </button>
            ))}
          </Section>

          <Section title={`Sessions (${sessions.length})`}>
            {sessions.length === 0 && (
              <p className="px-2 text-[11px] text-neutral-600">
                Start a replay to stream events
              </p>
            )}
            {sessions.map((id, i) => (
              <button
                key={id}
                onClick={() => setActive(id)}
                className={`flex w-full items-center gap-2 rounded px-2 py-1 text-left text-[12px] ${
                  active === id
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-400 hover:bg-neutral-900"
                }`}
              >
                <span
                  className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                    active === id ? "bg-emerald-400" : "bg-neutral-600"
                  }`}
                />
                <span className="truncate font-mono text-[11px]">
                  session {i + 1} · {id.slice(0, 8)}
                </span>
              </button>
            ))}
          </Section>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col">
          <div className="flex h-8 shrink-0 items-center border-b border-neutral-800 px-3 text-[11px] text-neutral-500">
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

      <footer className="flex h-6 shrink-0 items-center gap-3 border-t border-neutral-800 px-3 text-[10px] text-neutral-600">
        <span>Sessions: {sessions.length}</span>
        <span>Watching: {active ? 1 : 0}</span>
        <span>Last seq: {stats.lastSeq}</span>
      </footer>
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="border-b border-neutral-800 p-2">
      <h2 className="mb-1 px-2 text-[10px] font-medium tracking-wider text-neutral-500 uppercase">
        {title}
      </h2>
      <div className="space-y-0.5">{children}</div>
    </div>
  );
}
