import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";
import { cn } from "../../lib/utils";

/**
 * Variants map to consequence, not to decoration.
 *
 * `primary` starts work, `danger` stops an agent mid-turn, `ghost` navigates. Someone scanning
 * this file should be able to tell how much a button costs to press without reading its label,
 * and nothing merely emphatic is allowed to borrow the destructive treatment.
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1.5 rounded-md font-medium whitespace-nowrap transition-all duration-150 disabled:pointer-events-none disabled:opacity-40 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        primary:
          "bg-deck-text text-deck-void shadow-[0_1px_2px_oklch(0_0_0/0.4)] hover:bg-white active:translate-y-px",
        secondary:
          "border border-white/10 bg-white/5 text-deck-text hover:border-white/20 hover:bg-white/10 active:translate-y-px",
        ghost: "text-deck-dim hover:bg-white/6 hover:text-deck-text",
        danger:
          "border border-deck-danger/40 bg-deck-danger/12 text-deck-danger hover:bg-deck-danger/22 active:translate-y-px",
        attention:
          "bg-deck-attention text-deck-void shadow-[0_1px_2px_oklch(0_0_0/0.4)] hover:brightness-110 active:translate-y-px",
      },
      size: {
        sm: "h-6 px-2 text-[11px] [&_svg]:size-3",
        md: "h-7 px-2.5 text-[12px] [&_svg]:size-3.5",
        lg: "h-8 px-3 text-[13px] [&_svg]:size-4",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp ref={ref} className={cn(buttonVariants({ variant, size }), className)} {...props} />
    );
  },
);
Button.displayName = "Button";
