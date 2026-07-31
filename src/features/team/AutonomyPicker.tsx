import { useState } from "react";
import type { Autonomy } from "../../lib/types";

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
      <div className="flex rounded border border-neutral-700">
        {MODES.map((mode) => (
          <button
            key={mode.id}
            onClick={() => select(mode.id)}
            disabled={disabled}
            title={mode.description}
            className={`px-2 py-0.5 text-[11px] first:rounded-l last:rounded-r disabled:opacity-40 ${
              value === mode.id
                ? "bg-neutral-100 font-medium text-neutral-900"
                : "text-neutral-400 hover:text-neutral-200"
            }`}
          >
            {mode.label}
          </button>
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
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
      <div className="w-[26rem] rounded border border-neutral-700 bg-neutral-900 p-4">
        <h2 className="text-[13px] font-medium text-neutral-100">Run without approvals?</h2>
        <p className="mt-2 text-[12px] leading-relaxed text-neutral-400">
          Agents will start on their own, edit files and run shell commands in their worktrees,
          and retry failed work — with nobody watching. You will still be asked about anything
          that reaches outside a worktree.
        </p>
        <div className="mt-3 flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="rounded px-2.5 py-1 text-[12px] text-neutral-400 hover:text-neutral-200"
          >
            Cancel
          </button>
          <button
            onClick={onConfirm}
            className="rounded bg-amber-600 px-2.5 py-1 text-[12px] font-medium text-neutral-950 hover:bg-amber-500"
          >
            Run unattended
          </button>
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
  const colour =
    mode === "autonomous"
      ? "bg-amber-500"
      : mode === "assisted"
        ? "bg-sky-600"
        : "bg-neutral-600";
  return <div className={`h-0.5 w-full shrink-0 ${colour}`} title={`${mode} mode`} />;
}
