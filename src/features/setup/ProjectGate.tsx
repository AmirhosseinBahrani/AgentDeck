import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Boxes, FolderGit2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import type { ProjectInfo } from "../../lib/types";

/**
 * Stands in front of the app until there is a repository to work in.
 *
 * The project used to be inferred from the working directory, which is fine from a terminal and
 * useless from Finder — a `.app` launched by double-click inherits `/`, so worktrees, the
 * planner and verification all pointed at the root of the disk. Asking is the only thing that
 * works in both cases, and the answer is remembered so it is asked once.
 *
 * A gate rather than a banner because every single thing the app does needs somewhere to put a
 * worktree. There is no useful subset to show first.
 */
export function ProjectGate({ children }: { children: React.ReactNode }) {
  const [project, setProject] = useState<ProjectInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const read = useCallback(async () => {
    try {
      setProject(await invoke<ProjectInfo>("get_project"));
    } catch {
      setProject({ path: null, name: null });
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  const choose = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const picked = await open({ directory: true, multiple: false, title: "Choose a repository" });
      if (typeof picked === "string") {
        setProject(await invoke<ProjectInfo>("set_project", { path: picked }));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  // Nothing on screen during the first read: it takes a moment, and flashing a setup screen at
  // someone whose project is already known would be worse than a brief blank.
  if (!project) {
    return <div className="h-full" />;
  }

  if (project.path) {
    return <>{children}</>;
  }

  return (
    <div className="flex h-full flex-col items-center justify-center px-8">
      <div className="animate-rise w-full max-w-lg">
        <div className="mb-5 flex items-center gap-2 text-deck-live">
          <Boxes className="size-5" />
          <span className="text-[14px] font-semibold tracking-tight">AgentDeck</span>
        </div>

        <h1 className="text-[17px] leading-snug font-semibold">Choose a repository</h1>
        <p className="mt-1.5 text-[12.5px] leading-relaxed text-deck-dim">
          Agents work in git worktrees branched from it, one per task, so their changes stay
          isolated from each other and from your checkout until you merge them.
        </p>

        <div className="glass mt-5 flex items-center gap-3 rounded-[var(--radius-panel)] p-4">
          <FolderGit2 className="size-4 shrink-0 text-deck-faint" />
          <span className="grow text-[12px] text-deck-dim">
            It has to be a git repository — that is where the worktrees go.
          </span>
          <Button variant="primary" onClick={choose} disabled={busy}>
            {busy ? "Opening…" : "Choose folder"}
          </Button>
        </div>

        {error && <div className="mt-3 text-[11.5px] text-deck-danger">{error}</div>}
      </div>
    </div>
  );
}

/** Opens the picker from anywhere else in the app, e.g. the title bar. */
export async function pickProject(): Promise<ProjectInfo | null> {
  const picked = await open({ directory: true, multiple: false, title: "Choose a repository" });
  if (typeof picked !== "string") return null;
  return invoke<ProjectInfo>("set_project", { path: picked });
}
