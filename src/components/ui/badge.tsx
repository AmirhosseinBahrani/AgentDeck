import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";
import { cn } from "../../lib/utils";

const badgeVariants = cva(
  "inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium",
  {
    variants: {
      tone: {
        neutral: "bg-deck-raised text-deck-dim",
        live: "bg-deck-live/14 text-deck-live",
        attention: "bg-deck-attention/16 text-deck-attention",
        danger: "bg-deck-danger/16 text-deck-danger",
        done: "bg-deck-done/14 text-deck-done",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

export function Badge({
  className,
  tone,
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}
