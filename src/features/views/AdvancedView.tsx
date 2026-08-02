import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { SectionRule } from "../../components/ui/section-rule";
import { cn } from "../../lib/utils";

interface AgentMetrics {
  agent_id: string;
  name: string;
  role: string;
  sessions: number;
  tasks_completed: number;
  tasks_failed: number;
  attempts: number;
  review_rounds: number;
  cost_usd: number;
  turns: number;
  last_active_ms: number | null;
}

/**
 * What the team has actually done, per agent, across every run on this project.
 *
 * The roster answers "who is working right now" and forgets everything the moment a run ends.
 * This is the other question — whether a particular agent is worth its slot. The two numbers that
 * carry the most are attempts against tasks completed, which is rework, and cost, which is what a
 * second agent of the same role actually costs to keep.
 */
export function AdvancedView({ project }: { project: string | null }) {
  const [rows, setRows] = useState<AgentMetrics[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setRows(await invoke<AgentMetrics[]>("agent_metrics"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const id = setInterval(() => void load(), 5000);
    return () => clearInterval(id);
  }, [load, project]);

  const byRole = new Map<string, AgentMetrics[]>();
  for (const row of rows) {
    byRole.set(row.role, [...(byRole.get(row.role) ?? []), row]);
  }

  const total = rows.reduce(
    (acc, r) => ({
      cost: acc.cost + r.cost_usd,
      done: acc.done + r.tasks_completed,
      failed: acc.failed + r.tasks_failed,
      sessions: acc.sessions + r.sessions,
    }),
    { cost: 0, done: 0, failed: 0, sessions: 0 },
  );

  return (
    <div className="flex min-h-0 grow flex-col gap-6 overflow-y-auto px-7 pt-[18px] pb-[26px]">
      <Models project={project} />

      <Permissions project={project} />

      <div className="flex flex-col gap-1">
        <SectionRule label="Team metrics" trailing={project ?? undefined} />
        <p className="text-[11.5px] leading-relaxed text-deck-faint">
          Every run on this project, not just the current one. Attempts above tasks completed is
          rework.
        </p>
      </div>

      {error && <p className="text-[11.5px] text-deck-danger">{error}</p>}

      <div className="flex flex-wrap gap-2">
        <Stat label="Agents" value={String(rows.length)} />
        <Stat label="Roles" value={String(byRole.size)} />
        <Stat label="Sessions" value={String(total.sessions)} />
        <Stat label="Completed" value={String(total.done)} />
        <Stat label="Failed" value={String(total.failed)} tone={total.failed > 0 ? "warn" : undefined} />
        <Stat label="Spent" value={`$${total.cost.toFixed(2)}`} />
      </div>

      {rows.length === 0 ? (
        <p className="text-[11.5px] leading-relaxed text-deck-faint">
          Nothing recorded yet. Numbers appear once agents have run.
        </p>
      ) : (
        [...byRole.entries()]
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([role, members]) => (
            <section key={role} className="flex flex-col gap-1.5">
              <SectionRule
                label={role}
                trailing={`${members.length} agent${members.length === 1 ? "" : "s"}`}
              />
              <table className="w-full border-separate border-spacing-0 text-left">
                <thead>
                  <tr className="label-micro">
                    <Th className="w-[26%]">Agent</Th>
                    <Th numeric>Sessions</Th>
                    <Th numeric>Turns</Th>
                    <Th numeric>Done</Th>
                    <Th numeric>Failed</Th>
                    <Th numeric>Attempts</Th>
                    <Th numeric>Reviews</Th>
                    <Th numeric>Cost</Th>
                    <Th numeric>Last active</Th>
                  </tr>
                </thead>
                <tbody>
                  {members
                    .sort((a, b) => a.name.localeCompare(b.name))
                    .map((m) => (
                      <tr key={m.agent_id} className="hover:bg-deck-surface">
                        <Td className="text-deck-text">{m.name}</Td>
                        <Td numeric>{m.sessions}</Td>
                        <Td numeric>{m.turns}</Td>
                        <Td numeric>{m.tasks_completed}</Td>
                        <Td numeric tone={m.tasks_failed > 0 ? "warn" : undefined}>
                          {m.tasks_failed}
                        </Td>
                        <Td
                          numeric
                          // Flagged only when it exceeds what was delivered: attempts equal to
                          // completions is one clean run each, which is the good case.
                          tone={m.attempts > m.tasks_completed ? "warn" : undefined}
                        >
                          {m.attempts}
                        </Td>
                        <Td numeric>{m.review_rounds}</Td>
                        <Td numeric>${m.cost_usd.toFixed(2)}</Td>
                        <Td numeric>{ago(m.last_active_ms)}</Td>
                      </tr>
                    ))}
                </tbody>
              </table>
            </section>
          ))
      )}
    </div>
  );
}

const MODELS: { id: string; label: string; note: string }[] = [
  { id: "", label: "CLI default", note: "Whatever your installed Claude Code picks" },
  { id: "claude-opus-5", label: "Opus 5", note: "Newest" },
  { id: "claude-opus-4-8", label: "Opus 4.8", note: "" },
  { id: "claude-opus-4-7", label: "Opus 4.7", note: "" },
  { id: "claude-fable-5", label: "Fable 5", note: "" },
  { id: "claude-sonnet-4-6", label: "Sonnet 4.6", note: "The usual balance" },
  { id: "claude-haiku-4-5-20251001", label: "Haiku 4.5", note: "Fastest and cheapest" },
];

/**
 * Which model works and which supervises.
 *
 * Two settings rather than one because the roles have different shapes. Workers run long duplex
 * sessions doing the engineering; the supervisor makes short schema-constrained decisions —
 * planning, choosing an assignee, judging a review. A weaker supervisor is a false economy, since
 * a bad plan wastes every worker's time downstream, while a weaker worker costs only its own task.
 */
function Models({ project }: { project: string | null }) {
  const [worker, setWorker] = useState("");
  const [supervisor, setSupervisor] = useState("");
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void invoke<{ worker: string; supervisor: string }>("get_models")
      .then((m) => {
        setWorker(m.worker);
        setSupervisor(m.supervisor);
      })
      .catch(() => {});
  }, [project]);

  async function persist(nextWorker: string, nextSupervisor: string) {
    setWorker(nextWorker);
    setSupervisor(nextSupervisor);
    try {
      await invoke("save_models", {
        settings: { worker: nextWorker, supervisor: nextSupervisor },
      });
      setStatus("Saved. Applies to the next run — a running agent's model was fixed when it started.");
    } catch (e) {
      setStatus(String(e));
    }
  }

  return (
    <section className="flex flex-col gap-2.5">
      <SectionRule label="Models" trailing={project ?? undefined} />

      <div className="grid grid-cols-2 gap-4">
        <ModelPicker
          label="Agents"
          hint="Runs the actual work."
          value={worker}
          onChange={(v) => void persist(v, supervisor)}
        />
        <ModelPicker
          label="Supervisor"
          hint="Plans, assigns and judges reviews."
          value={supervisor}
          onChange={(v) => void persist(worker, v)}
        />
      </div>

      {status && <p className="text-[11px] leading-relaxed text-deck-dim">{status}</p>}
    </section>
  );
}

function ModelPicker({
  label,
  hint,
  value,
  onChange,
}: {
  label: string;
  hint: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const known = MODELS.some((m) => m.id === value);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline gap-2">
        <span className="label-micro">{label}</span>
        <span className="text-[10.5px] text-deck-faint">{hint}</span>
      </div>
      <div className="flex flex-col gap-1">
        {MODELS.map((model) => (
          <button
            key={model.id || "default"}
            onClick={() => onChange(model.id)}
            className={cn(
              "flex items-center gap-2.5 rounded-md border px-2.5 py-1.5 text-left transition-colors",
              value === model.id
                ? "border-deck-accent/40 bg-deck-accent-wash"
                : "border-deck-line bg-deck-surface hover:bg-deck-raised",
            )}
          >
            <span
              className={cn(
                "size-2 shrink-0 rounded-full border",
                value === model.id ? "border-deck-accent bg-deck-accent" : "border-deck-faint/60",
              )}
            />
            <span
              className={cn(
                "shrink-0 text-[12px]",
                value === model.id ? "text-deck-text" : "text-deck-dim",
              )}
            >
              {model.label}
            </span>
            <span className="min-w-0 truncate text-[10.5px] text-deck-faint">{model.note}</span>
          </button>
        ))}

        {/* Model ids change faster than this app ships, and an unrecognised one is rejected by
            the CLI at spawn rather than silently ignored — so a free field is safer than a list
            that quietly goes stale. */}
        <Input
          value={known ? "" : value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="Or type a model id"
          className="mt-1"
        />
      </div>
    </div>
  );
}

const LEVELS: { id: string; label: string; description: string }[] = [
  {
    id: "cautious",
    label: "Cautious",
    description:
      "Reads and searches freely. Every write and every shell command is asked about first.",
  },
  {
    id: "standard",
    label: "Standard",
    description:
      "Edits inside its own worktree without asking. Shell is limited to a known list of build, test and git commands.",
  },
  {
    id: "trusted",
    label: "Trusted",
    description:
      "As Standard, plus any shell command that the refuse-list does not catch.",
  },
];

/**
 * How much the team may do here without asking.
 *
 * One axis rather than a tool-by-tool grid, because the operator's actual question is how far
 * they trust agents on this codebase. What the axis cannot move is stated rather than hidden: the
 * worktree boundary is enforced by the CLI, and the refuse-list is unioned across policy layers
 * and cannot be overridden downstream — so a control that appeared to relax either would be
 * claiming something the resolver does not honour.
 */
function Permissions({ project }: { project: string | null }) {
  const [level, setLevel] = useState("standard");
  const [extra, setExtra] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    void invoke<{ level: string; extra_bash: string[] }>("get_permissions")
      .then((p) => {
        setLevel(p.level);
        setExtra(p.extra_bash);
      })
      .catch(() => {});
  }, [project]);

  async function persist(nextLevel: string, nextExtra: string[]) {
    setLevel(nextLevel);
    setExtra(nextExtra);
    try {
      await invoke("save_permissions", {
        settings: { level: nextLevel, extra_bash: nextExtra },
      });
      setStatus("Saved. Applies to the next run — agents already working keep the rules they started under.");
    } catch (e) {
      setStatus(String(e));
    }
  }

  return (
    <section className="flex flex-col gap-2.5">
      <SectionRule label="Permissions" trailing={project ?? undefined} />

      <div className="flex flex-col gap-1.5">
        {LEVELS.map((option) => (
          <button
            key={option.id}
            onClick={() => void persist(option.id, extra)}
            className={cn(
              "flex items-start gap-3 rounded-[var(--radius-panel)] border px-3.5 py-2.5 text-left transition-colors",
              level === option.id
                ? "border-deck-accent/40 bg-deck-accent-wash"
                : "border-deck-line bg-deck-surface hover:bg-deck-raised",
            )}
          >
            <span
              className={cn(
                "mt-[3px] size-2.5 shrink-0 rounded-full border",
                level === option.id
                  ? "border-deck-accent bg-deck-accent"
                  : "border-deck-faint/60",
              )}
            />
            <span className="flex min-w-0 flex-col gap-0.5">
              <span
                className={cn(
                  "text-[12.5px] font-medium",
                  level === option.id ? "text-deck-text" : "text-deck-dim",
                )}
              >
                {option.label}
              </span>
              <span className="text-[11.5px] leading-relaxed text-deck-faint">
                {option.description}
              </span>
            </span>
          </button>
        ))}
      </div>

      <p className="text-[11px] leading-relaxed text-deck-faint">
        At every level agents stay inside their own worktree, and{" "}
        <span className="font-mono text-deck-dim">rm -rf</span>,{" "}
        <span className="font-mono text-deck-dim">sudo</span>,{" "}
        <span className="font-mono text-deck-dim">git push</span> and network fetches are always
        refused. Those are not on this dial.
      </p>

      <div className="flex flex-col gap-1.5">
        <span className="label-micro">Always allow these commands</span>
        <div className="flex flex-wrap items-center gap-1.5">
          {extra.map((prefix) => (
            <span
              key={prefix}
              className="flex items-center gap-1.5 rounded-md border border-deck-line bg-deck-surface px-2 py-1 font-mono text-[11px] text-deck-dim"
            >
              {prefix}
              <button
                onClick={() => void persist(level, extra.filter((p) => p !== prefix))}
                className="text-deck-faint transition-colors hover:text-deck-attention"
              >
                ×
              </button>
            </span>
          ))}
          <Input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && draft.trim()) {
                void persist(level, [...extra, draft.trim()]);
                setDraft("");
              }
            }}
            placeholder="make test"
            className="w-[160px]"
          />
          {draft.trim() && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                void persist(level, [...extra, draft.trim()]);
                setDraft("");
              }}
            >
              Add
            </Button>
          )}
        </div>
      </div>

      {status && <p className="text-[11px] leading-relaxed text-deck-dim">{status}</p>}
    </section>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "warn" }) {
  return (
    <div className="flex min-w-[104px] flex-col gap-0.5 rounded-[var(--radius-panel)] border border-deck-line bg-deck-surface px-3.5 py-2.5">
      <span className="label-micro">{label}</span>
      <span
        className={cn(
          "font-mono text-[17px] tabular-nums",
          tone === "warn" ? "text-deck-attention" : "text-deck-text",
        )}
      >
        {value}
      </span>
    </div>
  );
}

function Th({
  children,
  numeric,
  className,
}: {
  children: React.ReactNode;
  numeric?: boolean;
  className?: string;
}) {
  return (
    <th
      className={cn(
        "border-b border-deck-line-strong pb-1.5 font-normal",
        numeric && "text-right",
        className,
      )}
    >
      {children}
    </th>
  );
}

function Td({
  children,
  numeric,
  tone,
  className,
}: {
  children: React.ReactNode;
  numeric?: boolean;
  tone?: "warn";
  className?: string;
}) {
  return (
    <td
      className={cn(
        "border-b border-deck-line py-[7px] text-[12px]",
        numeric && "text-right font-mono tabular-nums",
        tone === "warn" ? "text-deck-attention" : "text-deck-dim",
        className,
      )}
    >
      {children}
    </td>
  );
}

function ago(at: number | null): string {
  if (!at) return "—";
  const mins = Math.max(0, Math.round((Date.now() - at) / 60_000));
  if (mins < 1) return "now";
  if (mins < 60) return `${mins}m`;
  const hours = Math.round(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.round(hours / 24)}d`;
}
