import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { SectionRule } from "../../components/ui/section-rule";

/**
 * Standing notes about the project, given to every agent at spawn.
 *
 * This exists because agents run with `--setting-sources ''` and so never read the repository's
 * CLAUDE.md. That flag is deliberate — inheriting the operator's settings made workers cost four
 * times as much and behave unpredictably — but it left no way to tell the team anything true
 * about the codebase without restating it in every objective.
 *
 * Saved explicitly rather than as you type. What is written here is fed to real agents that spend
 * real budget, and autosaving would mean a half-finished sentence reaching the next spawn.
 */
export function MemoryView() {
  const [content, setContent] = useState("");
  const [saved, setSaved] = useState("");
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void invoke<string>("get_project_memory")
      .then((c) => {
        setContent(c);
        setSaved(c);
      })
      .catch(() => {});
  }, []);

  const dirty = content !== saved;

  async function save() {
    setBusy(true);
    setStatus(null);
    try {
      await invoke("save_project_memory", { content });
      setSaved(content);
      setStatus("Saved. Agents started from now on will be told this.");
    } catch (e) {
      setStatus(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-0 grow flex-col gap-4 px-7 pt-[18px] pb-[26px]">
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 flex-col gap-1">
          <SectionRule label="Project memory" />
          <p className="max-w-[62ch] text-[11.5px] leading-relaxed text-deck-faint">
            What every agent should already know about this codebase — conventions, things not to
            touch, decisions that are settled. Agents do not read your CLAUDE.md, so this is the
            only channel. It cannot grant permissions.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {dirty && (
            <span className="font-mono text-[10.5px] text-deck-attention">unsaved</span>
          )}
          <Button variant="primary" size="md" onClick={save} disabled={busy || !dirty}>
            Save
          </Button>
        </div>
      </div>

      <textarea
        value={content}
        onChange={(e) => setContent(e.target.value)}
        spellCheck={false}
        placeholder={"Migrations are hand-written — never generate them.\nThe API client in src/generated is produced by codegen; edit the schema instead."}
        className="min-h-0 grow resize-none rounded-[var(--radius-panel)] border border-white/[0.07] bg-white/[0.02] p-4 font-mono text-[12px] leading-[19px] text-deck-text placeholder:text-deck-faint/70 focus:border-white/[0.14] focus:outline-none"
      />

      {status && <p className="text-[11.5px] text-deck-dim">{status}</p>}
    </div>
  );
}
