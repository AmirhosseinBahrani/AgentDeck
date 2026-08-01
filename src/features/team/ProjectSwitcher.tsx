import { invoke } from "@tauri-apps/api/core";
import { Check, FolderGit2, FolderPlus, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";
import { useProjectPicker } from "../setup/ProjectGate";
import type { ProjectRow } from "../../lib/types";
import { cn } from "../../lib/utils";

/**
 * The repositories this workspace knows about, and which one the team is working in.
 *
 * A workspace holds several projects but only one is active: agents create worktrees inside the
 * current repository, and a team spread across two codebases would have no shared objective to
 * be a team about. Switching is therefore a change of context, not a filter.
 *
 * Missing directories are listed and marked rather than hidden. A project vanishing from the
 * list without explanation is more alarming than one that says it has moved.
 */
export function ProjectSwitcher({
  activePath,
  onSwitched,
  runActive,
}: {
  activePath: string | null;
  onSwitched: () => void;
  runActive: boolean;
}) {
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const picker = useProjectPicker(() => {
    onSwitched();
    void load();
  });

  const load = useCallback(async () => {
    try {
      setProjects(await invoke<ProjectRow[]>("list_projects"));
    } catch {
      setProjects([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, activePath]);

  async function switchTo(path: string) {
    setBusy(true);
    setError(null);
    try {
      await invoke("set_project", { path });
      onSwitched();
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="flex flex-col gap-1.5">
      <SectionRule
        label="Workspace"
        trailing={`${projects.length} project${projects.length === 1 ? "" : "s"}`}
        className="pb-2"
      />

      <div className="flex flex-col gap-0.5">
        {projects.map((project) => {
          const active = project.path === activePath;
          return (
            <button
              key={project.id}
              disabled={busy || active || !project.exists}
              onClick={() => switchTo(project.path)}
              title={project.path}
              className={cn(
                "flex h-8 items-center gap-2 rounded-md px-2 text-left transition-colors",
                active && "bg-deck-live/[0.09] ring-1 ring-deck-live/25",
                !active && project.exists && "hover:bg-white/[0.05]",
                !project.exists && "opacity-50",
              )}
            >
              {project.exists ? (
                <FolderGit2
                  className={cn(
                    "size-3.5 shrink-0",
                    active ? "text-deck-live" : "text-deck-faint",
                  )}
                />
              ) : (
                <TriangleAlert className="size-3.5 shrink-0 text-deck-attention" />
              )}
              <span
                className={cn(
                  "min-w-0 grow truncate text-[12.5px]",
                  active ? "font-semibold text-deck-text" : "text-deck-dim",
                )}
              >
                {project.name}
              </span>
              {!project.exists && (
                <span className="shrink-0 font-mono text-[10px] text-deck-attention">moved</span>
              )}
              {active && <Check className="size-3 shrink-0 text-deck-live" />}
            </button>
          );
        })}
      </div>

      <Button
        variant="ghost"
        size="sm"
        className="self-start"
        onClick={() => void picker.pick()}
        disabled={busy || picker.busy}
      >
        <FolderPlus /> Add a project
      </Button>
      {picker.dialog}

      {runActive && projects.length > 1 && (
        // Said before they try, not after it fails. Agents hold worktrees inside the current
        // repository, so switching mid-run would leave the roster and the processes describing
        // different codebases.
        <p className="px-0.5 text-[10.5px] leading-relaxed text-deck-faint">
          Stop the run to switch project — its agents are working in this repository.
        </p>
      )}

      {error && <p className="px-0.5 text-[11px] text-deck-danger">{error}</p>}
    </section>
  );
}
