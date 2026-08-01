import { invoke } from "@tauri-apps/api/core";
import { Bot, RefreshCw, Send, User } from "lucide-react";
import { useState } from "react";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";
import type { RunSnapshot } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The supervisor's reasoning, and where you shape it.
 *
 * Not a chat, despite the shape. The supervisor has no conversation — every decision is a
 * one-shot schema-validated call with a code-assembled prompt, which is what keeps decisions
 * replayable and stops state accumulating in a prompt across a run.
 *
 * What you write is stored as a standing instruction and folded into the *next* planning or
 * assignment prompt. That is what makes a free-text box safe here: guidance changes how work is
 * shaped — smaller tasks, a preferred test command, who owns what — but cannot widen
 * permissions, skip the verification gate, or mark anything done, because none of those read the
 * planner's prompt. They are enforced in code on the other side of it.
 */
export function SupervisorView({ snapshot }: { snapshot: RunSnapshot | null }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const decisions = snapshot?.decisions ?? [];
  const escalations = snapshot?.escalations ?? [];
  const guidance = snapshot?.guidance ?? [];
  const running = !!snapshot?.active;

  async function send(replan: boolean) {
    setBusy(true);
    setError(null);
    try {
      await invoke("send_guidance", { text, replan });
      setText("");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto flex h-full min-h-0 w-full max-w-3xl flex-col gap-4 overflow-y-auto px-7 py-5">
      <SectionRule
        label="Supervisor"
        trailing={snapshot?.objective ? `iteration ${snapshot.iteration}` : undefined}
      />

      {snapshot?.objective ? (
        <div className="card-row px-3.5 py-3">
          <div className="label-micro mb-1.5">Objective</div>
          <p className="text-[13px] leading-5 text-deck-text">{snapshot.objective}</p>
        </div>
      ) : (
        <p className="text-[11.5px] leading-relaxed text-deck-faint">
          No run. The supervisor's reasoning appears here once one starts.
        </p>
      )}

      {guidance.map((g) => (
        <Turn key={g.id} tone="neutral" who="You" meta={`iteration ${g.given_at_iteration}`}>
          <p className="text-[12.5px] leading-[18px] text-deck-text">{g.text}</p>
          {g.replan && (
            <p className="mt-1 text-[11px] text-deck-faint">Asked for the plan to be redone.</p>
          )}
        </Turn>
      ))}

      {escalations.map((e) => (
        <Turn key={e.id} tone="attention" who="Asking you">
          <p className="text-[13px] leading-5 text-deck-text">{e.question}</p>
          {e.detail && (
            <pre className="mt-2 max-h-32 overflow-y-auto rounded bg-black/30 p-2 font-mono text-[10.5px] leading-relaxed whitespace-pre-wrap text-deck-dim">
              {e.detail}
            </pre>
          )}
          <p className="mt-2 text-[11px] text-deck-faint">
            Answer it on the Team view — the options there are the ones the run will accept.
          </p>
        </Turn>
      ))}

      {decisions.map((d, i) => (
        <Turn
          key={i}
          tone={d.decided_by === "claude" ? "live" : "neutral"}
          who={d.decided_by === "claude" ? "Claude" : d.decided_by === "human" ? "You" : "Code"}
          meta={`${d.stage} · iteration ${d.iteration}${d.repaired ? " · repaired" : ""}`}
        >
          <p className="text-[12.5px] leading-[18px] font-medium text-deck-text">
            {d.kind.replace(/_/g, " ")}
          </p>
          {d.rationale && (
            <p className="mt-1 text-[12px] leading-[18px] text-deck-dim">{d.rationale}</p>
          )}
        </Turn>
      ))}

      <div className="sticky bottom-0 mt-2 flex shrink-0 flex-col gap-2 rounded-[var(--radius-panel)] border border-white/[0.08] bg-deck-base/95 p-3 backdrop-blur-xl">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends, Shift+Enter breaks the line. Guidance is usually one sentence.
            if (e.key === "Enter" && !e.shiftKey && text.trim() && !busy) {
              e.preventDefault();
              void send(false);
            }
          }}
          rows={2}
          disabled={!running}
          placeholder={
            running
              ? "Break tasks down further · always run pytest, not unittest · give the reviewer the schema work"
              : "Start a run before guiding it."
          }
          className="w-full resize-none rounded-md border border-white/10 bg-black/30 px-3 py-2 text-[12.5px] leading-relaxed text-deck-text placeholder:text-deck-faint focus:border-deck-live/50 focus:outline-none disabled:opacity-50"
        />
        <div className="flex items-center gap-2">
          {/* Says where guidance reaches, next to the box. Somebody will type "skip the tests",
              and it is better that they know beforehand that it will not do that. */}
          <span className="grow text-[10.5px] leading-relaxed text-deck-faint">
            Applied to the next planning and assignment decisions. It cannot grant permissions or
            bypass the verification gate.
          </span>
          <Button
            variant="secondary"
            size="sm"
            disabled={!running || busy || !text.trim()}
            onClick={() => void send(true)}
            title="Store this and decompose the objective again from scratch"
          >
            <RefreshCw /> Send and replan
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={!running || busy || !text.trim()}
            onClick={() => void send(false)}
          >
            <Send /> Send
          </Button>
        </div>
        {error && <p className="text-[11px] text-deck-danger">{error}</p>}
      </div>
    </div>
  );
}

function Turn({
  who,
  meta,
  tone,
  children,
}: {
  who: string;
  meta?: string;
  tone: "live" | "attention" | "neutral";
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-3">
      <span
        className={cn(
          "mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-md border",
          tone === "live" && "border-deck-live/30 bg-deck-live/[0.12] text-deck-live",
          tone === "attention" &&
            "border-deck-attention/30 bg-deck-attention/[0.12] text-deck-attention",
          tone === "neutral" && "border-white/[0.08] bg-white/[0.04] text-deck-faint",
        )}
      >
        {who === "You" ? <User className="size-3.5" /> : <Bot className="size-3.5" />}
      </span>
      <div className="min-w-0 grow">
        <div className="flex items-baseline gap-2">
          <span
            className={cn(
              "text-[11.5px] font-medium",
              tone === "live" && "text-deck-live",
              tone === "attention" && "text-deck-attention",
              tone === "neutral" && "text-deck-dim",
            )}
          >
            {who}
          </span>
          {meta && <span className="font-mono text-[10px] text-deck-faint">{meta}</span>}
        </div>
        <div className="mt-1">{children}</div>
      </div>
    </div>
  );
}
