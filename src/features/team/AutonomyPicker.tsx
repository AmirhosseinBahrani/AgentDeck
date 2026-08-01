import { useState } from "react";
import { Button } from "../../components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "../../components/ui/tooltip";
import type { Autonomy } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * Chooses how much the supervisor may do without being asked.
 *
 * Assisted is the default because the operator should not discover that agents were running
 * unattended by finding out what they did. Entering Autonomous therefore confirms, and the
 * confirmation states the specific capability being granted rather than asking "are you sure" —
 * the thing being agreed to is that processes will edit files and run commands with nobody
 * watching, and that is what the sentence has to say.
 */
export function AutonomyPicker({
  value,
  onChange,
  disabled,
}: {
  value: Autonomy;
  onChange: (mode: Autonomy) => void;
  disabled: boolean;
}) {
  const [confirming, setConfirming] = useState(false);

  function select(mode: Autonomy) {
    if (mode === "autonomous" && value !== "autonomous") {
      setConfirming(true);
      return;
    }
    onChange(mode);
  }

  return (
    <div className="flex items-center gap-1.5">
      <div className="flex rounded-md border border-white/10 bg-black/20 p-0.5">
        {MODES.map((mode) => (
          <Tooltip key={mode.id}>
            <TooltipTrigger asChild>
              <button
                onClick={() => select(mode.id)}
                disabled={disabled}
                className={cn(
                  "rounded px-2 py-0.5 text-[11px] transition-colors disabled:opacity-40",
                  value === mode.id
                    ? // The selected mode is filled in its own semantic colour, so the level of
                      // autonomy is legible without reading the word.
                      mode.id === "autonomous"
                      ? "bg-deck-attention/90 font-medium text-deck-void"
                      : "bg-white/12 font-medium text-deck-text"
                    : "text-deck-faint hover:text-deck-dim",
                )}
              >
                {mode.label}
              </button>
            </TooltipTrigger>
            <TooltipContent>{mode.description}</TooltipContent>
          </Tooltip>
        ))}
      </div>

      {confirming && (
        <ConfirmAutonomous
          onCancel={() => setConfirming(false)}
          onConfirm={() => {
            setConfirming(false);
            onChange("autonomous");
          }}
        />
      )}
    </div>
  );
}

const MODES: { id: Autonomy; label: string; description: string }[] = [
  {
    id: "manual",
    label: "Manual",
    description:
      "Plans and assigns, but starts nothing and retries nothing. Every agent waits for you, and a failure comes straight back.",
  },
  {
    id: "assisted",
    label: "Assisted",
    description: "Plans, assigns and retries on its own. You approve each agent before it starts.",
  },
  {
    id: "autonomous",
    label: "Autonomous",
    description: "Runs unattended. You are told what happened rather than asked first.",
  },
];

function ConfirmAutonomous({
  onCancel,
  onConfirm,
}: {
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm">
      <div className="glass animate-rise w-[27rem] rounded-[var(--radius-panel)] p-4">
        <h2 className="text-[13px] font-medium text-deck-text">Run without approvals?</h2>
        <p className="mt-2 text-[12px] leading-relaxed text-deck-dim">
          Agents will start on their own, edit files and run shell commands in their worktrees,
          and retry failed work — with nobody watching. You will still be asked about anything
          that reaches outside a worktree.
        </p>
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="attention" onClick={onConfirm}>
            Run unattended
          </Button>
        </div>
      </div>
    </div>
  );
}

/**
 * A stripe along the top of the window, coloured by mode.
 *
 * Ambient rather than a badge: the question it answers — "can agents act right now without
 * asking me" — is one the operator needs answered while looking at something else entirely.
 */
export function AutonomyStripe({ mode, active }: { mode: Autonomy; active: boolean }) {
  if (!active) {
    return null;
  }
  // A lit edge rather than a flat bar: it reads as a status light on an instrument, and the
  // glow is what makes it noticeable in peripheral vision without occupying real space.
  const colour =
    mode === "autonomous"
      ? "bg-deck-attention shadow-[0_0_12px_2px_oklch(0.8_0.15_78/0.5)]"
      : mode === "assisted"
        ? "bg-deck-live shadow-[0_0_12px_2px_oklch(0.78_0.13_195/0.4)]"
        : "bg-deck-faint";
  return <div className={cn("h-px w-full shrink-0", colour)} title={`${mode} mode`} />;
}
