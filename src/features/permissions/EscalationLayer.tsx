import { invoke } from "@tauri-apps/api/core";
import { ShieldAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { useEscalations, type Escalation } from "../../hooks/useEscalations";
import { cn } from "../../lib/utils";

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
      <div className="flex h-8 shrink-0 items-center gap-2 border-b border-deck-attention/30 bg-deck-attention-wash px-3">
        <ShieldAlert className="size-3.5 shrink-0 text-deck-attention" />
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
    // Fixed, not absolute: this has to cover the viewport regardless of what it is nested in or
    // how far the transcript behind it has scrolled.
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-deck-text/25 p-6 backdrop-blur-sm">
      {/*
        Opaque, deliberately. This sits over a dense monospace transcript, and a translucent
        panel left the agent's own output legible straight through the question being asked —
        which is unreadable in exactly the moment that demands care. Blur alone is not enough
        behind high-contrast text.
      */}
      <div className="animate-rise flex max-h-[80vh] w-full max-w-xl flex-col overflow-hidden rounded-xl border border-deck-attention/25 bg-deck-bg shadow-[var(--deck-shadow)]">
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-deck-line bg-deck-attention-tint px-4 py-3">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-deck-text">
            <ShieldAlert className="size-4 text-deck-attention" />
            Permission required
          </h2>
          {/* A silent auto-deny is the worst failure mode here, so the deadline is always shown. */}
          {remaining !== null && (
            <span
              className={cn(
                "shrink-0 font-mono text-[11px] tabular-nums",
                expired && "text-deck-danger",
                !expired && remaining < 60_000 && "text-deck-attention",
                !expired && remaining >= 60_000 && "text-deck-faint",
              )}
            >
              {expired ? "declined — no response" : `auto-declines in ${formatRemaining(remaining)}`}
            </span>
          )}
        </div>

        <div className="min-h-0 flex-1 space-y-3.5 overflow-y-auto px-4 py-3.5">
          <Field label="Agent wants to use">
            {/* Full-strength text, not a state colour: nothing is running — this is the subject
                of the question, set against the dim values of the fields below it. */}
            <span className="font-mono text-deck-text">{escalation.tool}</span>
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
            <pre className="max-h-40 overflow-auto rounded border border-deck-line bg-deck-surface p-2.5 font-mono text-[11px] leading-relaxed break-all whitespace-pre-wrap text-deck-dim">
              {JSON.stringify(escalation.input, null, 2)}
            </pre>
          </Field>

          {escalation.suggestions.length > 0 && (
            <Field label="Claude suggests">
              <ul className="space-y-1">
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

        <div className="flex shrink-0 items-center gap-3 border-t border-deck-line bg-deck-surface px-4 py-3">
          {/* What allowing actually does, next to the button that does it. "Once" is the whole
              safety property here and it should not be something you have to already know. */}
          <span className="grow text-[10.5px] leading-relaxed text-deck-faint">
            Allowing applies to this one call. The agent asks again next time.
          </span>
          <Button variant="secondary" size="md" disabled={busy} onClick={() => void answer(false)}>
            Decline
          </Button>
          <Button variant="attention" size="md" disabled={busy} onClick={() => void answer(true)}>
            Allow once
          </Button>
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="label-micro mb-1">{label}</div>
      <div className="text-[12px] leading-relaxed">{children}</div>
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
    case "subcommandResults":
      return "The command contains a subcommand, so what it will actually run cannot be checked in advance.";
    case "otherPermissionRule":
      return "A permission rule requires approval for this.";
    default:
      // Humanised rather than shown raw. These come straight from the CLI and new ones appear
      // without warning — "subcommandResults" told an operator nothing about what to decide.
      return reasonType
        .replace(/([a-z])([A-Z])/g, "$1 $2")
        .replace(/^./, (c) => c.toUpperCase());
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
