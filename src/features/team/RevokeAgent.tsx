import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import type { AgentSummary, RevokeImpact } from "../../lib/types";

/**
 * Takes a member off the roster.
 *
 * The dialog leads with what the agent is holding — a live session, a task, a branch — because
 * the cost of removing someone is entirely in that, and asking for the decision without those
 * facts would be asking someone to guess.
 *
 * Deliberately reversible, and it says so. Revoking deactivates rather than deletes: the
 * worktree, the branch and every report and decision that referenced the agent survive, so
 * rehiring the same role picks the seat back up rather than starting a stranger.
 */
export function RevokeAgent({
  agent,
  onClose,
  onRevoked,
}: {
  agent: AgentSummary | null;
  onClose: () => void;
  onRevoked: () => void;
}) {
  const [impact, setImpact] = useState<RevokeImpact | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!agent) return;
    void invoke<RevokeImpact>("revoke_impact", { agentId: agent.id })
      .then(setImpact)
      .catch(() => setImpact(null));
  }, [agent]);

  if (!agent) return null;

  async function revoke() {
    setBusy(true);
    setError(null);
    try {
      await invoke<boolean>("revoke_agent", { agentId: agent!.id });
      onRevoked();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm">
      <div className="glass animate-rise flex w-[520px] flex-col rounded-[14px]">
        <header className="border-b border-white/[0.07] px-6 pt-5 pb-4">
          <div className="flex items-center gap-2.5">
            <h2 className="text-[19px] font-semibold tracking-tight text-deck-text">
              Revoke {agent.name}
            </h2>
            {agent.status === "blocked" && (
              <span className="flex h-5 items-center gap-1.5 rounded-full bg-deck-attention/[0.14] px-2">
                <span className="size-1.5 rounded-full bg-deck-attention" />
                <span className="text-[10px] font-semibold tracking-[0.06em] text-deck-attention">
                  BLOCKED
                </span>
              </span>
            )}
          </div>
          <p className="mt-1.5 text-[12.5px] leading-relaxed text-deck-dim">
            The agent leaves the roster and stops receiving work. Its history, reports and past
            diffs stay in the run record.
          </p>
        </header>

        <div className="flex flex-col gap-2.5 px-6 pt-5">
          <span className="label-micro">Work in flight</span>
          <Row n={impact?.live_sessions ?? 0} label="live session, stopped when you revoke" />
          <Row
            n={impact?.assigned_tasks.length ?? 0}
            label="assigned task, handed back to the supervisor"
            detail={impact?.assigned_tasks[0]}
          />
          {impact?.branch && (
            <Row n={1} label="worktree and branch, kept" detail={impact.branch} />
          )}
        </div>

        <div className="px-6 pt-5">
          {/* Not offered as a choice. Returning tasks to an unassigned backlog would stall the
              run until somebody noticed, whereas the supervisor reassigns on its next iteration
              and keeps the dependency graph intact. */}
          <p className="rounded-md border border-deck-live/25 bg-deck-live/[0.06] px-3 py-2.5 text-[11.5px] leading-relaxed text-deck-dim">
            Anything it was working on goes back to the supervisor, which reassigns it next
            iteration with the dependency graph intact. The branch and worktree are kept, so
            uncommitted work survives.
          </p>
          {error && <div className="mt-3 text-[11.5px] text-deck-danger">{error}</div>}
        </div>

        <footer className="mt-5 flex items-center gap-3 border-t border-white/[0.07] bg-white/[0.02] px-6 py-4">
          <span className="grow text-[11.5px] leading-relaxed text-deck-faint">
            Reversible — hiring this role again picks up the same seat and its history.
          </span>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="danger" onClick={revoke} disabled={busy}>
            {busy ? "Revoking…" : "Revoke access"}
          </Button>
        </footer>
      </div>
    </div>
  );
}

function Row({ n, label, detail }: { n: number; label: string; detail?: string }) {
  return (
    <div className="flex h-[26px] items-center gap-3">
      <span
        className={
          n > 0
            ? "w-[22px] shrink-0 font-mono text-[13px] text-deck-text"
            : "w-[22px] shrink-0 font-mono text-[13px] text-deck-faint"
        }
      >
        {n}
      </span>
      <span className="grow text-[12.5px] text-deck-dim">{label}</span>
      {detail && (
        <span className="max-w-[180px] shrink-0 truncate font-mono text-[10.5px] text-deck-faint">
          {detail}
        </span>
      )}
    </div>
  );
}
