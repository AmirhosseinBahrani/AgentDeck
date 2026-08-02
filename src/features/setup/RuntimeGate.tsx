import { invoke } from "@tauri-apps/api/core";
import { Boxes, RefreshCw, TerminalSquare } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import type { Readiness } from "../../lib/types";

/**
 * Stands in front of the app until the `claude` CLI is installed and logged in.
 *
 * Not a sign-in screen — AgentDeck has no accounts and needs no API key, because spawned agents
 * inherit OAuth credentials from the OS keychain. A login form here would be inventing a concept
 * the product does not have, and would imply AgentDeck holds credentials it never sees.
 *
 * It is a gate rather than a warning banner because every single thing the app does requires a
 * working CLI. Letting someone write an objective and press Start, only to watch an agent fail to
 * spawn minutes later, moves the blame onto their objective.
 *
 * The fix always happens outside this window, so there is a re-check button and no restart.
 */
export function RuntimeGate({ children }: { children: React.ReactNode }) {
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [checking, setChecking] = useState(true);

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setReadiness(await invoke<Readiness>("check_runtime"));
    } catch (e) {
      setReadiness({ state: "unknown", detail: String(e) });
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check]);

  // Nothing on screen during the first check. It takes a few hundred milliseconds, and flashing
  // a setup screen at someone whose machine is fine would be worse than a brief blank.
  if (!readiness) {
    return <div className="h-full" />;
  }

  if (readiness.state === "ready") {
    return <>{children}</>;
  }

  return (
    <div className="flex h-full flex-col items-center justify-center px-8 text-deck-text">
      <div className="animate-rise w-full max-w-lg">
        {/* Accent, not the live teal: this is the product's mark, and nothing is running yet. */}
        <div className="mb-5 flex items-center gap-2 text-deck-accent">
          <Boxes className="size-5" />
          <span className="text-[14px] font-semibold tracking-tight">AgentDeck</span>
        </div>

        <h1 className="text-[17px] leading-snug font-semibold">
          One thing to set up first
        </h1>
        <p className="mt-1.5 text-[12px] leading-relaxed text-deck-dim">
          Agents are <code className="font-mono text-deck-text">claude</code> processes. AgentDeck
          runs and supervises them — there is no account here and it never sees your credentials.
        </p>

        <div className="glass mt-5 rounded-[var(--radius-panel)] p-4">
          <Problem readiness={readiness} />
        </div>

        <div className="mt-4 flex items-center gap-3">
          <Button variant="primary" size="lg" onClick={() => void check()} disabled={checking}>
            <RefreshCw className={checking ? "animate-spin" : undefined} />
            {checking ? "Checking…" : "Check again"}
          </Button>
          <span className="text-[11px] leading-relaxed text-deck-faint">
            Run the command in a terminal, then check again — no restart needed.
          </span>
        </div>
      </div>
    </div>
  );
}

function Problem({ readiness }: { readiness: Readiness }) {
  switch (readiness.state) {
    case "not_installed":
      return (
        <Fix
          title={`\`${readiness.program}\` is not on your PATH`}
          detail="Install the CLI, then reopen or re-check."
          command="npm install -g @anthropic-ai/claude-code"
        />
      );

    case "not_authenticated":
      return (
        <Fix
          title="The CLI is installed but nobody has logged in"
          detail={`Found ${readiness.version}. Logging in happens in the CLI; AgentDeck reads nothing but whether it succeeded.`}
          command="claude auth login"
        />
      );

    // Deliberately distinct from "not installed". Telling someone to reinstall something that is
    // already there wastes their time and hides the real error.
    case "unknown":
      return (
        <Fix
          title="The CLI is there, but the check did not complete"
          detail={readiness.detail}
          command="claude auth status"
        />
      );

    default:
      return null;
  }
}

function Fix({
  title,
  detail,
  command,
}: {
  title: string;
  detail: string;
  command: string;
}) {
  return (
    <>
      <div className="text-[13px] font-medium text-deck-attention">{title}</div>
      <p className="mt-1.5 text-[11.5px] leading-relaxed text-deck-dim">{detail}</p>
      {/* The command is the whole point of this screen, so it is set as one: selectable, in
          monospace, visually separated from the prose explaining it. */}
      <div className="mt-3 flex items-center gap-2 rounded-md border border-deck-line bg-deck-surface px-2.5 py-2">
        <TerminalSquare className="size-3.5 shrink-0 text-deck-faint" />
        {/* Full-strength text, not a colour: the command is content to copy, and the inset
            monospace block already separates it from the prose. */}
        <code className="font-mono text-[11.5px] text-deck-text select-all">{command}</code>
      </div>
    </>
  );
}
