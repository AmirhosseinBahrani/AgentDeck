import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { RecoveryReport, ResumableSummary } from "../../lib/types";

/**
 * What the last launch left behind.
 *
 * Shown rather than logged because the alternative is silence about something the operator has
 * every reason to be wrong about: after a force-quit they will assume their agents are still
 * working. They are not — startup killed them — and the sessions they were in can be reopened,
 * but only from the directories they ran in. None of that is discoverable without being told.
 *
 * Dismissible and gone for the rest of the launch. It describes a moment, not a state, so
 * leaving it on screen would make it furniture.
 */
export function RecoveryBanner() {
  const [report, setReport] = useState<RecoveryReport | null>(null);
  const [resumable, setResumable] = useState<ResumableSummary[]>([]);
  const [dismissed, setDismissed] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const [r, sessions] = await Promise.all([
          invoke<RecoveryReport>("get_startup_recovery"),
          invoke<ResumableSummary[]>("get_resumable_sessions"),
        ]);
        setReport(r);
        setResumable(sessions);
      } catch {
        // Nothing to recover is indistinguishable from a failed query here, and the banner is
        // not worth an error of its own.
      }
    })();
  }, []);

  const nothingHappened =
    !report ||
    (report.killed_orphans === 0 &&
      report.interrupted_sessions === 0 &&
      resumable.length === 0);

  if (dismissed || nothingHappened) {
    return null;
  }

  async function resume(sessionId: string) {
    setBusy(sessionId);
    setError(null);
    try {
      await invoke<string>("resume_session", { sessionId });
      setResumable((prev) => prev.filter((s) => s.session_id !== sessionId));
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="shrink-0 border-b border-amber-900/60 bg-amber-950/25 px-4 py-2 text-[11px]">
      <div className="flex items-baseline gap-2">
        <span className="font-medium text-amber-200">AgentDeck did not shut down cleanly</span>
        <span className="text-amber-400/80">{describe(report)}</span>
        <button
          onClick={() => setDismissed(true)}
          className="ml-auto text-amber-500/70 hover:text-amber-300"
        >
          Dismiss
        </button>
      </div>

      {resumable.length > 0 && (
        <ul className="mt-1.5 space-y-1">
          {resumable.map((session) => (
            <li key={session.session_id} className="flex items-center gap-2">
              <span className="font-mono text-[10px] text-neutral-400">
                {session.session_id.slice(0, 8)}
              </span>
              <span className="min-w-0 flex-1 truncate text-neutral-500">{session.cwd}</span>
              {session.resumable ? (
                <button
                  onClick={() => void resume(session.session_id)}
                  disabled={busy === session.session_id}
                  className="rounded border border-amber-800/70 px-1.5 py-0.5 text-amber-200 hover:bg-amber-900/40 disabled:opacity-50"
                >
                  {busy === session.session_id ? "Reopening…" : "Reopen"}
                </button>
              ) : (
                // Stated rather than left to a failed click: the worktree is gone, and Claude
                // buckets conversations by directory, so this one is unreachable for good.
                <span
                  className="text-neutral-600"
                  title="Its worktree was removed, which deletes the conversation with it"
                >
                  worktree gone
                </span>
              )}
            </li>
          ))}
        </ul>
      )}

      {error && <div className="mt-1 text-red-400">{error}</div>}
    </div>
  );
}

function describe(report: RecoveryReport): string {
  const parts: string[] = [];
  if (report.killed_orphans > 0) {
    parts.push(
      `${report.killed_orphans} agent${report.killed_orphans === 1 ? "" : "s"} were still running and have been stopped`,
    );
  }
  if (report.interrupted_sessions > 0) {
    parts.push(`${report.interrupted_sessions} session${report.interrupted_sessions === 1 ? "" : "s"} interrupted`);
  }
  return parts.join(" · ");
}
