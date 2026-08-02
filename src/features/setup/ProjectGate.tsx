import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Boxes, FolderGit2, GitBranchPlus } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import type { FolderInfo, ProjectInfo } from "../../lib/types";

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

  const [pending, setPending] = useState<FolderInfo | null>(null);

  const choose = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const folder = await pickFolder();
      if (!folder) return;
      if (folder.is_repository && folder.has_commits) {
        setProject(await openFolder(folder.path));
      } else {
        setPending(folder);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  const initialise = useCallback(async (folder: FolderInfo) => {
    setBusy(true);
    setError(null);
    try {
      setProject(await initFolder(folder.path));
      setPending(null);
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
        {/* Accent, not the live teal: this is the product's mark, and nothing is running yet. */}
        <div className="mb-5 flex items-center gap-2 text-deck-accent">
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

        {pending && (
          <ConfirmInit
            folder={pending}
            busy={busy}
            onCancel={() => setPending(null)}
            onConfirm={() => void initialise(pending)}
          />
        )}

        {error && <div className="mt-3 text-[11.5px] text-deck-danger">{error}</div>}
      </div>
    </div>
  );
}

/**
 * Asks before writing to a folder the operator picked.
 *
 * Says exactly what will happen, including the empty commit — which is not incidental. A fresh
 * repository has no HEAD, and `git worktree add` has nothing to branch from, so without it the
 * first dispatch would fail with an error about an invalid reference far from this moment.
 */
export function ConfirmInit({
  folder,
  busy,
  onCancel,
  onConfirm,
}: {
  folder: FolderInfo;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <div className="glass animate-rise mt-4 rounded-[var(--radius-panel)] p-4">
      <div className="flex items-start gap-2.5">
        <GitBranchPlus className="mt-px size-4 shrink-0 text-deck-attention" />
        <div className="min-w-0">
          <p className="text-[13px] font-medium text-deck-text">
            {folder.is_repository
              ? `${folder.name} has no commits yet`
              : `${folder.name} is not a git repository`}
          </p>
          <p className="mt-1 text-[11.5px] leading-relaxed text-deck-dim">
            {folder.is_repository
              ? "Agents branch a worktree per task, and there is nothing to branch from yet."
              : "Agents work in git worktrees, so this folder needs to be a repository first."}
          </p>
          <code className="mt-2 block truncate rounded bg-deck-surface px-2 py-1.5 font-mono text-[10.5px] text-deck-faint">
            {folder.path}
          </code>
          <p className="mt-2 text-[11px] leading-relaxed text-deck-faint">
            This will run{" "}
            <span className="font-mono text-deck-dim">
              {folder.is_repository ? "git commit --allow-empty" : "git init"}
            </span>{" "}
            in that folder. Nothing else is touched.
          </p>
        </div>
      </div>
      <div className="mt-3 flex justify-end gap-2">
        <Button variant="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
        <Button variant="attention" onClick={onConfirm} disabled={busy}>
          {busy ? "Setting up…" : folder.is_repository ? "Create a base commit" : "Initialise it"}
        </Button>
      </div>
    </div>
  );
}

/**
 * Opens the picker from anywhere else in the app, e.g. the title bar.
 *
 * Returns the folder rather than opening it when it needs initialising, so the caller can ask
 * first. `git init` writes into a directory the operator picked, and doing that silently because
 * the folder happened not to be a repository would be taking the decision for them.
 */
export async function pickFolder(): Promise<FolderInfo | null> {
  const picked = await open({ directory: true, multiple: false, title: "Choose a folder" });
  if (typeof picked !== "string") return null;
  return invoke<FolderInfo>("inspect_folder", { path: picked });
}

/** Opens a folder that is already a usable repository. */
export function openFolder(path: string): Promise<ProjectInfo> {
  return invoke<ProjectInfo>("set_project", { path });
}

/** Makes a folder into a repository and opens it. Only call once the operator has agreed. */
export function initFolder(path: string): Promise<ProjectInfo> {
  return invoke<ProjectInfo>("init_project", { path });
}

/**
 * The picker plus its confirmation, as one flow.
 *
 * Shared by the gate and the title bar so both behave identically — a folder that needs
 * initialising must ask in either place, not only on first launch.
 */
export async function pickProject(
  confirmInit: (folder: FolderInfo) => Promise<boolean>,
): Promise<ProjectInfo | null> {
  const folder = await pickFolder();
  if (!folder) return null;
  if (folder.is_repository && folder.has_commits) {
    return openFolder(folder.path);
  }
  return (await confirmInit(folder)) ? initFolder(folder.path) : null;
}

/**
 * The picker flow, reusable wherever a project can be chosen.
 *
 * A hook rather than duplicated state because the title bar and the project list both offer
 * this, and a folder needing initialisation must ask in either place — the moment one of them
 * skipped the question, `git init` would happen silently depending on which button was pressed.
 */
export function useProjectPicker(onOpened: (project: ProjectInfo) => void) {
  const [pending, setPending] = useState<FolderInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pick = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const folder = await pickFolder();
      if (!folder) return;
      if (folder.is_repository && folder.has_commits) {
        onOpened(await openFolder(folder.path));
      } else {
        setPending(folder);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [onOpened]);

  const confirm = useCallback(async () => {
    if (!pending) return;
    setBusy(true);
    setError(null);
    try {
      onOpened(await initFolder(pending.path));
      setPending(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [pending, onOpened]);

  const dialog = pending ? (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-deck-text/25 px-8 backdrop-blur-sm">
      <div className="w-full max-w-md">
        <ConfirmInit
          folder={pending}
          busy={busy}
          onCancel={() => setPending(null)}
          onConfirm={() => void confirm()}
        />
        {error && <p className="mt-2 text-[11.5px] text-deck-danger">{error}</p>}
      </div>
    </div>
  ) : null;

  return { pick, dialog, busy, error };
}
