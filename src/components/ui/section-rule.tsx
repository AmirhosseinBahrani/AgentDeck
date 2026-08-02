import * as React from "react";
import { cn } from "../../lib/utils";

/**
 * A section heading with a rule running to the right edge.
 *
 * Used everywhere in place of a boxed panel header. On a dense instrument panel a full border
 * around every group turns the screen into a grid of boxes; a label and a hairline separate
 * content just as clearly and leave the space for data.
 */
export function SectionRule({
  label,
  trailing,
  accent,
  className,
}: {
  label: string;
  trailing?: React.ReactNode;
  accent?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("flex items-center gap-3 px-0.5", className)}>
      <span className={cn("label-micro", accent && "text-deck-attention")}>{label}</span>
      <div className="h-px grow bg-deck-line" />
      {trailing && <span className="text-[11.5px] text-deck-faint">{trailing}</span>}
    </div>
  );
}
