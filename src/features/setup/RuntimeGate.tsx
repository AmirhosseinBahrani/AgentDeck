import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
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
    return <div className="h-full bg-neutral-950" />;
  }

  if (readiness.state === "ready") {
    return <>{children}</>;
  }

  return (
    <div className="flex h-full flex-col items-center justify-center bg-neutral-950 px-8 text-neutral-200">
      <div className="w-full max-w-lg">
        <h1 className="text-[15px] font-semibold text-neutral-100">
          AgentDeck needs the Claude Code CLI
        </h1>
        <p className="mt-1 text-[12px] text-neutral-500">
          Agents are <code className="text-neutral-400">claude</code> processes. AgentDeck runs and
          supervises them — it never handles your credentials.
        </p>

        <div className="mt-4 rounded border border-neutral-800 bg-neutral-900/60 p-3">
          <Problem readiness={readiness} />
        </div>

        <div className="mt-3 flex items-center gap-2">
          <button
            onClick={() => void check()}
            disabled={checking}
            className="rounded bg-neutral-100 px-2.5 py-1 text-[12px] font-medium text-neutral-900 hover:bg-white disabled:opacity-40"
          >
            {checking ? "Checking…" : "Check again"}
          </button>
          <span className="text-[11px] text-neutral-600">
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
      <div className="text-[13px] text-amber-300">{title}</div>
      <p className="mt-1 text-[11px] leading-relaxed text-neutral-400">{detail}</p>
      <code className="mt-2 block rounded bg-neutral-950 px-2 py-1.5 font-mono text-[11px] text-neutral-300">
        {command}
      </code>
    </>
  );
}
