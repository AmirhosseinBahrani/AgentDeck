import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useEscalations, type Escalation } from "../../hooks/useEscalations";

/**
 * Blocking permission prompts.
 *
 * Mounted at the app shell, outside any route or session panel, because an agent can block
 * while the operator is looking at something else entirely. A prompt buried inside the
 * transcript of a non-visible session would silently time out.
 */
export function EscalationLayer() {
  const { open, resolve } = useEscalations();
  const current = open[0];

  if (!current) return null;

  return (
    <>
      <div className="flex h-8 shrink-0 items-center justify-between border-b border-deck-attention/30 bg-deck-attention/12 px-3">
        <span className="text-[12px] text-deck-attention">
          {open.length === 1
            ? "An agent needs your decision"
            : `${open.length} agents need your decision`}
        </span>
      </div>
      <PermissionDialog escalation={current} onResolve={resolve} />
    </>
  );
}

function PermissionDialog({
  escalation,
  onResolve,
}: {
  escalation: Escalation;
  onResolve: (requestId: string, allow: boolean) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remaining = useCountdown(escalation.expiresAtMs);

  async function answer(allow: boolean) {
    setBusy(true);
    setError(null);
    try {
      await invoke("respond_permission", {
        requestId: escalation.requestId,
        allow,
        updatedInput: allow ? escalation.input : null,
      });
      onResolve(escalation.requestId, allow);
    } catch (e) {
      // Never optimistic: if the backend rejected the answer the agent is still blocked, and
      // pretending otherwise would strand it silently.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const expired = remaining !== null && remaining <= 0;

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/60 p-6">
      <div className="w-full max-w-xl rounded-lg border border-white/12 bg-white/4 shadow-2xl">
        <div className="flex items-center justify-between border-b border-white/8 px-4 py-2.5">
          <h2 className="text-[13px] font-semibold text-deck-text">Permission required</h2>
          {/* A silent auto-deny is the worst failure mode here, so the deadline is always shown. */}
          {remaining !== null && (
            <span
              className={`font-mono text-[11px] ${
                expired ? "text-deck-danger" : remaining < 60_000 ? "text-deck-attention" : "text-deck-faint"
              }`}
            >
              {expired ? "declined — no response" : `auto-declines in ${formatRemaining(remaining)}`}
            </span>
          )}
        </div>

        <div className="space-y-3 px-4 py-3">
          <Field label="Agent wants to use">
            <span className="font-mono text-deck-live">{escalation.tool}</span>
          </Field>

          {escalation.blockedPath && (
            <Field label="Blocked path">
              <span className="font-mono break-all text-deck-dim">
                {escalation.blockedPath}
              </span>
            </Field>
          )}

          {escalation.reasonType && (
            <Field label="Reason">
              <span className="text-deck-dim">{describeReason(escalation.reasonType)}</span>
            </Field>
          )}

          <Field label="Input">
            <pre className="max-h-40 overflow-auto rounded bg-black/30 p-2 font-mono text-[11px] text-deck-dim">
              {JSON.stringify(escalation.input, null, 2)}
            </pre>
          </Field>

          {escalation.suggestions.length > 0 && (
            <Field label="Claude suggests">
              <ul className="space-y-0.5">
                {escalation.suggestions.map((s, i) => (
                  <li key={i} className="text-[11px] text-deck-dim">
                    {/* Rendered from the CLI's structured suggestion, never parsed from prose. */}
                    {describeSuggestion(s)}
                  </li>
                ))}
              </ul>
            </Field>
          )}

          {error && (
            <p className="rounded border border-deck-danger/35 bg-deck-danger/12 px-2 py-1.5 text-[11px] text-deck-danger">
              Could not apply your answer: {error}
            </p>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 border-t border-white/8 px-4 py-2.5">
          <button
            disabled={busy}
            onClick={() => void answer(false)}
            className="rounded border border-white/12 px-3 py-1.5 text-[12px] text-deck-dim hover:bg-white/8 disabled:opacity-50"
          >
            Decline
          </button>
          <button
            disabled={busy}
            onClick={() => void answer(true)}
            className="rounded-md bg-deck-attention px-3 py-1.5 text-[12px] font-medium text-deck-void transition-all hover:brightness-110 active:translate-y-px disabled:opacity-50"
          >
            Allow once
          </button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-0.5 text-[10px] tracking-wider text-deck-faint uppercase">{label}</div>
      <div className="text-[12px]">{children}</div>
    </div>
  );
}

/** Ticks once a second. Cheap because at most one dialog is mounted at a time. */
function useCountdown(expiresAtMs: number | null): number | null {
  const [remaining, setRemaining] = useState(() =>
    expiresAtMs === null ? null : expiresAtMs - Date.now(),
  );

  useEffect(() => {
    if (expiresAtMs === null) return;
    setRemaining(expiresAtMs - Date.now());
    const id = setInterval(() => setRemaining(expiresAtMs - Date.now()), 1000);
    return () => clearInterval(id);
  }, [expiresAtMs]);

  return remaining;
}

function formatRemaining(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return m > 0 ? `${m}m ${String(s).padStart(2, "0")}s` : `${s}s`;
}

/** Turns the CLI's classification into something an operator can act on. */
function describeReason(reasonType: string): string {
  switch (reasonType) {
    case "workingDir":
      return "The path is outside this agent's working directory.";
    case "permissionPromptTool":
      return "This tool always requires approval.";
    case "sandboxOverride":
      return "The command would run outside the sandbox.";
    default:
      return reasonType;
  }
}

function describeSuggestion(suggestion: unknown): string {
  const s = suggestion as { type?: string; directories?: string[]; rules?: unknown[] };
  if (s?.type === "addDirectories" && s.directories) {
    return `Grant access to ${s.directories.join(", ")} for this session`;
  }
  if (s?.type === "addRules") {
    return `Add ${s.rules?.length ?? 0} permanent allow rule(s)`;
  }
  return JSON.stringify(suggestion);
}
