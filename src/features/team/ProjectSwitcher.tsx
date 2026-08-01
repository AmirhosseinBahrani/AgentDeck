import { invoke } from "@tauri-apps/api/core";
import { ArrowUp, Check, FolderGit2, FolderPlus, Sparkles, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
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
  onStarted,
  runActive,
}: {
  activePath: string | null;
  onSwitched: () => void;
  /** Called with the first objective after a project is created, so the run starts immediately. */
  onStarted: (objective: string) => void;
  runActive: boolean;
}) {
  const [creating, setCreating] = useState(false);
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

      {creating ? (
        <NewProject
          onCancel={() => setCreating(false)}
          onCreated={(objective) => {
            setCreating(false);
            onSwitched();
            void load();
            if (objective.trim()) onStarted(objective.trim());
          }}
        />
      ) : (
        <div className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setCreating(true)}
            disabled={busy || runActive}
          >
            <Sparkles /> New project
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void picker.pick()}
            disabled={busy || picker.busy}
          >
            <FolderPlus /> Add existing
          </Button>
        </div>
      )}
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

/**
 * Describe something, get a repository with a team already working on it.
 *
 * The two halves are one action on purpose. A new project and its first objective are the same
 * thought, and splitting them into "create a folder" then "now say what to build" makes the
 * operator state their intention twice. The description is optional — creating an empty project
 * to point at later is legitimate — but the field is the primary one because starting work is
 * the reason to make a project at all.
 */
function NewProject({
  onCancel,
  onCreated,
}: {
  onCancel: () => void;
  onCreated: (objective: string) => void;
}) {
  const [name, setName] = useState("");
  const [objective, setObjective] = useState("");
  const [location, setLocation] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Shown before committing to it. The path is derived from the name by rules the operator cannot
  // see, and finding out where a repository landed after the fact is a poor way to learn them.
  useEffect(() => {
    if (!name.trim()) {
      setLocation("");
      return;
    }
    void invoke<string>("suggest_project_location", { name })
      .then(setLocation)
      .catch(() => setLocation(""));
  }, [name]);

  async function create() {
    setBusy(true);
    setError(null);
    try {
      await invoke("create_project", { name });
      onCreated(objective);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-2 rounded-[var(--radius-panel)] border border-white/[0.08] bg-white/[0.025] p-3">
      <Input
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="Project name"
      />

      <div className="flex items-end gap-2">
        <textarea
          value={objective}
          onChange={(e) => setObjective(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends, as it does everywhere else a message is composed. Shift+Enter for a
            // newline, since an objective worth writing often needs two lines.
            if (e.key === "Enter" && !e.shiftKey && name.trim() && !busy) {
              e.preventDefault();
              void create();
            }
          }}
          rows={2}
          placeholder="What should the team build first? (optional)"
          className="min-w-0 grow resize-none rounded-md border border-white/[0.08] bg-white/[0.03] px-2.5 py-2 text-[12.5px] leading-[18px] text-deck-text placeholder:text-deck-faint focus:border-white/[0.16] focus:outline-none"
        />
        <Button
          variant="primary"
          size="md"
          onClick={() => void create()}
          disabled={busy || !name.trim()}
          title="Create the repository and start the team on it"
        >
          <ArrowUp />
        </Button>
      </div>

      {location && (
        <p className="truncate px-0.5 font-mono text-[10.5px] text-deck-faint" title={location}>
          {location}
        </p>
      )}

      {error && <p className="px-0.5 text-[11px] leading-relaxed text-deck-danger">{error}</p>}

      <button
        onClick={onCancel}
        className="self-start px-0.5 text-[11px] text-deck-faint transition-colors hover:text-deck-dim"
      >
        Cancel
      </button>
    </div>
  );
}
