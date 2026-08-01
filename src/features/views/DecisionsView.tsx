import { SectionRule } from "../../components/ui/section-rule";
import type { RunSnapshot } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * Every decision the run has made, and who made it.
 *
 * The `decided_by` column is the point of this screen. The design's central claim is that the
 * supervisor is a deterministic controller with the model confined to bounded decision points,
 * and this is where that claim is checkable: a run where everything says "code" means the model
 * kept producing unusable answers and fallbacks carried it, which is invisible anywhere else.
 */
export function DecisionsView({ snapshot }: { snapshot: RunSnapshot | null }) {
  const decisions = snapshot?.decisions ?? [];
  const byModel = decisions.filter((d) => d.decided_by === "claude").length;
  const repaired = decisions.filter((d) => d.repaired).length;

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 overflow-y-auto px-7 py-5">
      <SectionRule
        label="Decisions"
        trailing={
          decisions.length > 0
            ? `${byModel} by Claude · ${decisions.length - byModel} by code`
            : undefined
        }
      />

      {repaired > 0 && (
        // Surfaced rather than buried per row: a run with many repairs is one where the model
        // keeps answering in a shape the validator rejects, and that is a prompt problem worth
        // noticing in aggregate.
        <p className="text-[11.5px] text-deck-attention">
          {repaired} decision{repaired === 1 ? "" : "s"} needed a repair round-trip before they
          could be applied.
        </p>
      )}

      {decisions.length === 0 ? (
        <p className="text-[11.5px] leading-relaxed text-deck-faint">
          Nothing decided yet. Every choice lands here — what was decided, why, and whether code,
          Claude or you made it.
        </p>
      ) : (
        <div className="flex flex-col">
          {decisions.map((d, i) => (
            <div
              key={i}
              className="flex gap-4 border-b border-white/[0.05] py-2.5 last:border-b-0"
            >
              <span className="w-10 shrink-0 pt-0.5 font-mono text-[10.5px] text-deck-faint">
                #{d.iteration}
              </span>
              <span
                className={cn(
                  "w-14 shrink-0 pt-0.5 font-mono text-[10.5px]",
                  d.decided_by === "claude" && "text-deck-live",
                  d.decided_by === "human" && "text-deck-attention",
                  d.decided_by === "code" && "text-deck-faint",
                )}
              >
                {d.decided_by}
              </span>
              <div className="flex min-w-0 grow flex-col gap-0.5">
                <span className="text-[12.5px] leading-[18px] font-medium text-deck-text">
                  {d.kind.replace(/_/g, " ")}
                </span>
                {d.rationale && (
                  <span className="text-[11.5px] leading-[17px] text-deck-dim">
                    {d.rationale}
                  </span>
                )}
              </div>
              <span className="w-20 shrink-0 pt-0.5 text-right font-mono text-[10px] text-deck-faint">
                {d.stage}
              </span>
              {d.repaired && (
                <span className="w-14 shrink-0 pt-0.5 text-right font-mono text-[10px] text-deck-attention">
                  repaired
                </span>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
