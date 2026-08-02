import * as React from "react";
import { cn } from "../../lib/utils";

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  ({ className, ...props }, ref) => (
    <input
      ref={ref}
      className={cn(
        "w-full rounded-md border border-deck-line bg-deck-surface px-3 py-2 text-deck-text",
        "placeholder:text-deck-faint",
        // Focus is interaction, so it takes cobalt rather than the running-agent teal it used to.
        "transition-colors focus:border-deck-accent focus:outline-none",
        className,
      )}
      {...props}
    />
  ),
);
Input.displayName = "Input";
