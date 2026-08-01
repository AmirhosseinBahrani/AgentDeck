import { Bot, Lock } from "lucide-react";
import { SectionRule } from "../../components/ui/section-rule";
import type { RunSnapshot } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The supervisor's reasoning, read as a thread.
 *
 * Read-only, and that is a design decision rather than an unfinished feature.
 *
 * The supervisor has no long-lived model session. Every decision is a one-shot call against a
 * schema with a code-assembled prompt, and its memory is the database — which is what makes
 * decisions replayable and stops an unconstrained prompt accumulating state across a run. A
 * message box wired into it would reintroduce exactly that, and would be a free-text channel
 * into the permission model, which is the reason escalations were made a typed enum in the
 * first place.
 *
 * So this renders what the supervisor decided and why. When it needs something from you it
 * asks, with the answers it will accept — on the Team view, where you can actually act.
 */
export function SupervisorView({ snapshot }: { snapshot: RunSnapshot | null }) {
  const decisions = snapshot?.decisions ?? [];
  const escalations = snapshot?.escalations ?? [];

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

      {/* Where a composer would be. Saying why there isn't one beats leaving a gap that reads
          as something unbuilt. */}
      <div className="mt-2 flex shrink-0 items-start gap-2.5 rounded-[var(--radius-panel)] border border-white/[0.07] bg-white/[0.02] px-3.5 py-3">
        <Lock className="mt-px size-3.5 shrink-0 text-deck-faint" />
        <p className="text-[11.5px] leading-relaxed text-deck-faint">
          You cannot message the supervisor. It has no ongoing conversation to join — every
          decision is a separate call against a fixed schema, which is what keeps them replayable
          and stops one long prompt drifting over a run. When it needs you it raises a decision
          with the specific answers it will accept.
        </p>
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
        <Bot className="size-3.5" />
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
