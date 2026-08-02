import { AlertTriangle } from "lucide-react";
import { Button } from "../../components/ui/button";
import type { Escalation, EscalationAnswer } from "../../lib/types";

/**
 * The questions the run is waiting on, with the answers it will accept.
 *
 * This replaces a dead end. The dashboard used to say "waiting for a decision from you" and
 * offer nothing to decide — the escalation was a counter with no question attached, and the loop
 * had already exited, so the promise that it would resume was false twice over.
 *
 * Options are rendered from what the supervisor sends, never composed here. Code decides which
 * answers are legal for a given failure, so the UI cannot offer a button that does nothing — and
 * there is no free-text field, because a prose channel into the supervisor is exactly the hole
 * the permission model exists to close.
 */
export function EscalationInbox({
  escalations,
  onAnswer,
  busy,
}: {
  escalations: Escalation[];
  onAnswer: (id: string, answer: EscalationAnswer) => void;
  busy: string | null;
}) {
  return (
    <>
      {escalations.map((escalation) => (
        <div
          key={escalation.id}
          className="animate-rise rounded-[7px] border border-deck-attention/30 bg-deck-attention-wash p-2.5"
        >
          <div className="flex items-start gap-2">
            <AlertTriangle className="mt-px size-3.5 shrink-0 text-deck-attention" />
            <div className="min-w-0 flex-1">
              <div className="text-[12px] leading-snug font-medium text-deck-text">
                {escalation.question}
              </div>
              {escalation.detail && (
                // Whatever produced the failure, in its own words. Truncated rather than
                // scrolled: a wall of test output would bury the buttons under it.
                <pre className="mt-1.5 max-h-24 overflow-y-auto rounded bg-deck-bg p-1.5 font-mono text-[10px] leading-relaxed whitespace-pre-wrap text-deck-dim">
                  {escalation.detail}
                </pre>
              )}
            </div>
          </div>

          <div className="mt-2 flex flex-wrap gap-1.5">
            {escalation.options.map((option) => (
              <Button
                key={option.label}
                size="sm"
                variant={option.destructive ? "danger" : "secondary"}
                title={option.consequence}
                disabled={busy === escalation.id}
                onClick={() => onAnswer(escalation.id, option.answer)}
              >
                {option.label}
              </Button>
            ))}
          </div>

          {/* The consequence of each option, spelled out rather than left to a tooltip nobody
              hovers. These answers abandon work or end runs. */}
          <ul className="mt-1.5 space-y-0.5">
            {escalation.options.map((option) => (
              <li key={option.label} className="text-[10px] leading-relaxed text-deck-faint">
                <span className="text-deck-dim">{option.label}</span> — {option.consequence}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </>
  );
}
