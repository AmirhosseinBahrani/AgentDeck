import * as React from "react";
import { cn } from "../../lib/utils";

/**
 * A titled glass column.
 *
 * Takes a `count` and an `accent` because in this app a panel's header is the fastest place to
 * learn that something needs you — "Waiting for you 3" in amber is legible from across a desk,
 * where the same information inside the list is not.
 */
export function Panel({
  title,
  count,
  accent,
  actions,
  children,
  className,
}: {
  title: string;
  count?: number;
  accent?: "live" | "attention";
  actions?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("flex min-h-0 flex-col", className)}>
      <header className="flex h-8 shrink-0 items-center gap-2 border-b border-deck-line px-3">
        <h2
          className={cn(
            "label-micro",
            accent === "attention" && "text-deck-attention",
            accent === "live" && "text-deck-live",
          )}
        >
          {title}
        </h2>
        {count !== undefined && count > 0 && (
          <span
            className={cn(
              "rounded px-1 font-mono text-[10px] leading-4",
              accent === "attention"
                ? "bg-deck-attention/18 text-deck-attention"
                : "bg-deck-raised text-deck-dim",
            )}
          >
            {count}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1">{actions}</div>
      </header>
      <div className="min-h-0 flex-1 space-y-1.5 overflow-y-auto p-2">{children}</div>
    </section>
  );
}

/** Shown in place of a list. Says what would appear here, not just that nothing has. */
export function Empty({ children }: { children: React.ReactNode }) {
  return (
    <p className="px-1 py-2 text-[11px] leading-relaxed text-deck-faint">{children}</p>
  );
}
