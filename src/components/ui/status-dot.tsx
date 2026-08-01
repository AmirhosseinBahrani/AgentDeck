import { cn } from "../../lib/utils";

/**
 * Shape and motion, never colour alone.
 *
 * Colour is already carrying per-role identity elsewhere, and roughly one in twelve of the
 * engineers this is built for cannot separate the red from the green. So a running agent
 * pulses, a queued one is a hollow ring, and a finished one is a flat disc — each state is
 * distinguishable with the colour removed entirely.
 */
const STYLES: Record<string, string> = {
  running: "bg-deck-live animate-live ring-live",
  review: "bg-deck-live/70",
  completed: "bg-deck-done",
  failed: "bg-deck-danger",
  cancelled: "bg-deck-faint/50",
  blocked: "bg-deck-attention ring-attention",
  assigned: "bg-deck-dim",
  queued: "border border-deck-dim/70",
  backlog: "border border-deck-faint/50",
};

export function StatusDot({ status, className }: { status: string; className?: string }) {
  return (
    <span
      title={status}
      className={cn(
        "size-1.5 shrink-0 rounded-full",
        STYLES[status] ?? "bg-deck-faint",
        className,
      )}
    />
  );
}
