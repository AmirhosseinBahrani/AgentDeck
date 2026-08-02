import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, File, Folder, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";
import { cn } from "../../lib/utils";

interface FileEntry {
  path: string;
  name: string;
  is_dir: boolean;
  size: number;
}

/**
 * The project folder, as it actually is on disk.
 *
 * Every other view reports on the process — what agents say, what the supervisor decided, what
 * changed in a worktree. None of them answer the plainest question an operator has, which is
 * whether the thing they asked for exists yet. Work lives on branches inside worktrees until a
 * run integrates, so the folder can look untouched while a great deal has happened; being able
 * to look at it is what makes that legible instead of alarming.
 *
 * Refresh is manual. A run writes constantly, and a tree that reshuffled under the cursor while
 * being read would be worse than one that is briefly stale.
 */
export function FilesView({ project }: { project: string | null }) {
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [children, setChildren] = useState<Record<string, FileEntry[]>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [landing, setLanding] = useState(false);
  const [landed, setLanded] = useState<string | null>(null);

  const loadDir = useCallback(async (rel: string) => {
    try {
      const entries = await invoke<FileEntry[]>("list_project_files", { rel: rel || null });
      setChildren((prev) => ({ ...prev, [rel]: entries }));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const checkPending = useCallback(async () => {
    try {
      setPending(await invoke<boolean>("pending_integration"));
    } catch {
      setPending(false);
    }
  }, []);

  const refresh = useCallback(async () => {
    void checkPending();
    setChildren({});
    setOpen(new Set());
    setSelected(null);
    setContent(null);
    await loadDir("");
  }, [loadDir, checkPending]);

  useEffect(() => {
    void refresh();
  }, [refresh, project]);

  async function toggle(entry: FileEntry) {
    if (!entry.is_dir) {
      setSelected(entry.path);
      setContent(null);
      try {
        setContent(await invoke<string>("read_project_file", { rel: entry.path }));
      } catch (e) {
        setContent(String(e));
      }
      return;
    }

    const next = new Set(open);
    if (next.has(entry.path)) {
      next.delete(entry.path);
    } else {
      next.add(entry.path);
      if (!children[entry.path]) await loadDir(entry.path);
    }
    setOpen(next);
  }

  const root = children[""] ?? [];

  return (
    <div className="flex min-h-0 grow flex-col gap-3 px-7 pt-[18px] pb-[26px]">
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 flex-col gap-0.5">
          <SectionRule label="Files" trailing={project ?? undefined} />
          <p className="text-[11px] leading-relaxed text-deck-faint">
            The project folder itself. Agents work on branches in their own worktrees, so what you
            see here is only what has been integrated.
          </p>
        </div>
        <Button variant="ghost" size="sm" onClick={() => void refresh()}>
          <RefreshCw /> Refresh
        </Button>
      </div>

      {/* Placed here because this is the screen where its absence is felt: work that integrated
          but never landed leaves the folder looking as though the run produced nothing. */}
      {pending && (
        // Accent, not live: nothing is running here — the banner exists to offer the one action
        // on this screen.
        <div className="flex items-center gap-3 rounded-[var(--radius-panel)] border border-deck-accent/30 bg-deck-accent-wash px-3.5 py-2.5">
          <div className="flex min-w-0 grow flex-col gap-0.5">
            <span className="text-[12.5px] font-medium text-deck-text">
              There is integrated work that is not in this folder yet
            </span>
            <span className="text-[11.5px] leading-relaxed text-deck-faint">
              A run merged its branches and verified them, but stopped before landing — usually
              because it is still going, or a review needed you. Landing will not overwrite
              uncommitted changes or discard your own commits.
            </span>
          </div>
          <Button
            variant="primary"
            size="md"
            disabled={landing}
            onClick={async () => {
              setLanding(true);
              setError(null);
              setLanded(null);
              try {
                setLanded(await invoke<string>("land_integration"));
                await refresh();
              } catch (e) {
                setError(String(e));
              } finally {
                setLanding(false);
              }
            }}
          >
            {landing ? "Landing…" : "Land it"}
          </Button>
        </div>
      )}

      {landed && <p className="text-[11.5px] text-deck-done">{landed}</p>}
      {error && <p className="text-[11.5px] text-deck-danger">{error}</p>}

      <div className="flex min-h-0 grow gap-4">
        <div className="flex w-[300px] shrink-0 flex-col overflow-y-auto rounded-[var(--radius-panel)] border border-deck-line bg-deck-surface p-2">
          {root.length === 0 ? (
            <p className="px-2 py-1.5 text-[11.5px] leading-relaxed text-deck-faint">
              Nothing here yet. A run's work lands in this folder once its branches integrate.
            </p>
          ) : (
            <Tree
              entries={root}
              depth={0}
              open={open}
              children_={children}
              selected={selected}
              onToggle={toggle}
            />
          )}
        </div>

        <div className="flex min-w-0 grow flex-col">
          {selected ? (
            <>
              <span className="mb-1.5 truncate font-mono text-[11px] text-deck-faint">
                {selected}
              </span>
              <pre className="min-h-0 grow overflow-auto rounded-[var(--radius-panel)] border border-deck-line bg-deck-surface p-3 font-mono text-[11.5px] leading-[18px] text-deck-dim">
                {content ?? "Reading…"}
              </pre>
            </>
          ) : (
            <p className="text-[11.5px] leading-relaxed text-deck-faint">
              Pick a file to read it.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function Tree({
  entries,
  depth,
  open,
  children_,
  selected,
  onToggle,
}: {
  entries: FileEntry[];
  depth: number;
  open: Set<string>;
  children_: Record<string, FileEntry[]>;
  selected: string | null;
  onToggle: (entry: FileEntry) => void;
}) {
  return (
    <>
      {entries.map((entry) => (
        <div key={entry.path}>
          <button
            onClick={() => void onToggle(entry)}
            style={{ paddingLeft: `${depth * 12 + 6}px` }}
            className={cn(
              "flex h-[26px] w-full items-center gap-1.5 rounded-md pr-2 text-left transition-colors",
              selected === entry.path ? "bg-deck-accent-wash" : "hover:bg-deck-raised",
            )}
          >
            {entry.is_dir ? (
              open.has(entry.path) ? (
                <ChevronDown className="size-3 shrink-0 text-deck-faint" />
              ) : (
                <ChevronRight className="size-3 shrink-0 text-deck-faint" />
              )
            ) : (
              <span className="w-3 shrink-0" />
            )}
            {entry.is_dir ? (
              <Folder className="size-3 shrink-0 text-deck-dim" />
            ) : (
              <File className="size-3 shrink-0 text-deck-faint" />
            )}
            <span
              className={cn(
                "min-w-0 grow truncate text-[12px]",
                entry.is_dir ? "text-deck-dim" : "text-deck-text",
              )}
            >
              {entry.name}
            </span>
            {!entry.is_dir && (
              <span className="shrink-0 font-mono text-[10px] text-deck-faint">
                {size(entry.size)}
              </span>
            )}
          </button>

          {entry.is_dir && open.has(entry.path) && (
            <Tree
              entries={children_[entry.path] ?? []}
              depth={depth + 1}
              open={open}
              children_={children_}
              selected={selected}
              onToggle={onToggle}
            />
          )}
        </div>
      ))}
    </>
  );
}

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes}b`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)}k`;
  return `${(bytes / 1024 / 1024).toFixed(1)}m`;
}
