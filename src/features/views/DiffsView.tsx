import { invoke } from "@tauri-apps/api/core";
import { FileText, GitBranch, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";
import type { TaskDiff } from "../../lib/types";

/**
 * What the team has actually changed.
 *
 * The one view that reports on the work rather than the process. Every other screen shows what
 * agents *say* they are doing; this shows what is in their worktrees, which is the only claim
 * that cannot be talked around.
 *
 * Fetched on demand rather than polled. It costs a `git diff` per active worktree, and paying
 * that every second to render a tab that is usually closed would slow the whole app down.
 */
export function DiffsView() {
  const [diffs, setDiffs] = useState<TaskDiff[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setDiffs(await invoke<TaskDiff[]>("get_task_diffs"));
    } catch {
      setDiffs([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const withWork = (diffs ?? []).filter((d) => d.files.length > 0);
  const totals = withWork.reduce(
    (acc, d) => ({ added: acc.added + d.added, removed: acc.removed + d.removed }),
    { added: 0, removed: 0 },
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 overflow-y-auto px-7 py-5">
      <SectionRule
        label="Diffs"
        trailing={
          withWork.length > 0
            ? `+${totals.added} −${totals.removed} across ${withWork.length} worktree${withWork.length === 1 ? "" : "s"}`
            : undefined
        }
      />

      <div className="flex items-center gap-2">
        <Button variant="secondary" size="sm" onClick={() => void load()} disabled={loading}>
          <RefreshCw className={loading ? "animate-spin" : undefined} />
          {loading ? "Reading worktrees…" : "Refresh"}
        </Button>
        <span className="text-[11px] text-deck-faint">
          Read straight from each agent's worktree, including uncommitted work.
        </span>
      </div>

      {diffs === null ? null : withWork.length === 0 ? (
        <p className="text-[11.5px] leading-relaxed text-deck-faint">
          No agent has changed anything yet. Files appear here as soon as they are written, so
          you do not have to wait for a commit.
        </p>
      ) : (
        <div className="flex flex-col gap-2">
          {withWork.map((diff) => {
            const open = expanded === diff.task_id;
            return (
              <div key={diff.task_id} className="card-row overflow-hidden">
                <button
                  onClick={() => setExpanded(open ? null : diff.task_id)}
                  className="flex w-full items-center gap-3 px-3 py-2.5 text-left"
                >
                  <FileText className="size-3.5 shrink-0 text-deck-faint" />
                  <div className="flex min-w-0 grow flex-col gap-0.5">
                    <span className="truncate text-[12.5px] text-deck-text">{diff.title}</span>
                    <span className="flex items-center gap-1.5 truncate font-mono text-[10.5px] text-deck-faint">
                      {diff.role}
                      {diff.branch && (
                        <>
                          <GitBranch className="size-3 shrink-0" />
                          {diff.branch}
                        </>
                      )}
                      · {diff.files.length} file{diff.files.length === 1 ? "" : "s"}
                    </span>
                  </div>
                  <span className="shrink-0 font-mono text-[11px] text-deck-done">
                    +{diff.added}
                  </span>
                  <span className="shrink-0 font-mono text-[11px] text-deck-danger">
                    −{diff.removed}
                  </span>
                </button>

                {open && (
                  <div className="border-t border-white/[0.06] px-3 py-2">
                    {diff.files.map((file) => (
                      <div key={file.path} className="flex items-center gap-3 py-1">
                        <span className="min-w-0 grow truncate font-mono text-[11px] text-deck-dim">
                          {file.path}
                        </span>
                        {/* Uncommitted work is marked, because a file an agent is still editing
                            is not the same as one it has finished with. */}
                        {!file.committed && (
                          <span className="shrink-0 font-mono text-[9.5px] text-deck-attention">
                            uncommitted
                          </span>
                        )}
                        <span className="w-12 shrink-0 text-right font-mono text-[10.5px] text-deck-done">
                          +{file.added}
                        </span>
                        <span className="w-12 shrink-0 text-right font-mono text-[10.5px] text-deck-danger">
                          −{file.removed}
                        </span>
                        {/* Proportion, not absolute width: a 4000-line file would otherwise
                            flatten every other bar into invisibility. */}
                        <span className="flex h-1 w-16 shrink-0 overflow-hidden rounded-full bg-white/[0.08]">
                          <span
                            className="bg-deck-done"
                            style={{ width: `${share(file.added, file.added + file.removed)}%` }}
                          />
                          <span
                            className="bg-deck-danger"
                            style={{
                              width: `${share(file.removed, file.added + file.removed)}%`,
                            }}
                          />
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function share(part: number, total: number): number {
  return total === 0 ? 0 : Math.round((part / total) * 100);
}
