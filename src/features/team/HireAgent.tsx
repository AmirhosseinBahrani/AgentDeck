import { invoke } from "@tauri-apps/api/core";
import { UserPlus } from "lucide-react";
import { useState } from "react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import type { AgentRecord } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * Adds a member to the team.
 *
 * A role is free text because the supervisor assigns by matching a task's role against the
 * agents holding it — so naming a role is what makes it assignable, and constraining the list
 * to the three seeded ones would have meant a UI that could only ever describe what already
 * existed.
 *
 * Templates are starting points, not types. Picking one fills the form and then gets out of the
 * way; nothing downstream knows or cares which one was used.
 */
const TEMPLATES: { name: string; role: string; prompt: string }[] = [
  {
    name: "Database Engineer",
    role: "database",
    prompt:
      "Own schema, migrations and indexes. Never drop a column without raising a blocker. Every migration ships with a rollback.",
  },
  {
    name: "Backend Engineer",
    role: "developer",
    prompt: "Own service code and its tests. Prefer the smallest change that satisfies the contract.",
  },
  {
    name: "Frontend Engineer",
    role: "frontend",
    prompt: "Own the UI. Match existing components and tokens rather than introducing new ones.",
  },
  {
    name: "Docs Writer",
    role: "docs",
    prompt: "Own written material. Describe what the code does now, not what it was planned to do.",
  },
  { name: "", role: "", prompt: "" },
];

export function HireAgent({
  open,
  onClose,
  onHired,
}: {
  open: boolean;
  onClose: () => void;
  onHired: (agent: AgentRecord) => void;
}) {
  const [name, setName] = useState("");
  const [role, setRole] = useState("");
  const [prompt, setPrompt] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!open) return null;

  async function hire() {
    setBusy(true);
    setError(null);
    try {
      const agent = await invoke<AgentRecord>("hire_agent", {
        name,
        role: role || name,
        model: null,
        systemPrompt: prompt || null,
        mcpServers: [],
      });
      onHired(agent);
      setName("");
      setRole("");
      setPrompt("");
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm">
      <div className="glass animate-rise flex w-[560px] flex-col rounded-[14px]">
        <header className="border-b border-white/[0.07] px-6 pt-5 pb-4">
          <h2 className="text-[19px] font-semibold tracking-tight text-deck-text">
            Hire an agent
          </h2>
          <p className="mt-1.5 text-[12.5px] leading-relaxed text-deck-dim">
            A new member joins the roster permanently. The supervisor can assign it work from the
            next iteration.
          </p>
        </header>

        <div className="flex flex-col gap-4 px-6 py-5">
          <Field label="Role template">
            <div className="flex flex-wrap gap-1.5">
              {TEMPLATES.map((t, i) => (
                <button
                  key={i}
                  onClick={() => {
                    setName(t.name);
                    setRole(t.role);
                    setPrompt(t.prompt);
                  }}
                  className={cn(
                    "rounded-md border px-2.5 py-1 text-[11.5px] transition-colors",
                    name === t.name && t.name
                      ? "border-deck-live/40 bg-deck-live/[0.12] text-deck-live"
                      : "border-white/[0.09] bg-white/[0.04] text-deck-dim hover:text-deck-text",
                  )}
                >
                  {t.name || "Blank role"}
                </button>
              ))}
            </div>
          </Field>

          <div className="grid grid-cols-2 gap-4">
            <Field label="Name">
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Database Engineer"
                autoFocus
              />
            </Field>
            <Field
              label="Role"
              hint="What the supervisor assigns against. Tasks name a role; agents hold one."
            >
              <Input
                value={role}
                onChange={(e) => setRole(e.target.value)}
                placeholder="database"
              />
            </Field>
          </div>

          <Field
            label="Standing instructions"
            hint="Appended to every session this agent runs."
          >
            <textarea
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              rows={3}
              placeholder="Own schema, migrations and indexes. Never drop a column without raising a blocker."
              className="w-full resize-none rounded-md border border-white/10 bg-black/25 px-3 py-2 text-[12.5px] leading-relaxed text-deck-text placeholder:text-deck-faint focus:border-deck-live/50 focus:outline-none"
            />
          </Field>

          {/* Stated rather than configurable: containment is not per-agent policy. Every agent
              works in its own worktree and anything outside it goes through the permission
              prompt, which is what makes running them unattended defensible at all. */}
          <p className="rounded-md border border-white/[0.07] bg-white/[0.02] px-3 py-2 text-[11px] leading-relaxed text-deck-faint">
            Works in its own git worktree with edits auto-approved inside it. Anything reaching
            outside asks you first — the same containment every agent gets.
          </p>

          {error && <div className="text-[11.5px] text-deck-danger">{error}</div>}
        </div>

        <footer className="flex items-center gap-3 border-t border-white/[0.07] bg-white/[0.02] px-6 py-4">
          <span className="grow text-[11.5px] leading-relaxed text-deck-faint">
            Nothing starts until the supervisor assigns it a task.
          </span>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={hire} disabled={busy || !name.trim()}>
            <UserPlus /> {busy ? "Hiring…" : "Hire"}
          </Button>
        </footer>
      </div>
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="label-micro">{label}</span>
      {children}
      {hint && <span className="text-[10.5px] leading-relaxed text-deck-faint">{hint}</span>}
    </label>
  );
}
