import { invoke } from "@tauri-apps/api/core";
import { Download, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { SectionRule } from "../../components/ui/section-rule";
import type { Skill } from "../../lib/types";
import { cn } from "../../lib/utils";

const BLANK: Skill = { id: "", name: "", description: "", body: "", enabled: true };

/**
 * Named procedures the team is given at spawn.
 *
 * Separate from memory because each one is individually switchable, and that switch is the point:
 * a skill costs prompt tokens on every single spawn, so a library only stays affordable if the
 * ones that do not apply to the current work can be turned off without being deleted.
 */
export function SkillsView({ project }: { project: string | null }) {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [draft, setDraft] = useState<Skill | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setSkills(await invoke<Skill[]>("list_skills"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // Reloaded when the project changes: skills belong to a repository, and carrying one project's
  // list into another would let an operator toggle a skill they cannot see the effect of.
  useEffect(() => {
    setDraft(null);
    void load();
  }, [load, project]);

  /** Brings in `.claude/skills`, which agents cannot read for themselves. */
  async function importFromRepo() {
    setBusy(true);
    setError(null);
    try {
      const result = await invoke<{
        skills_imported: number;
        personal_skills_imported: number;
      }>("import_project_knowledge");
      await load();
      const total = result.skills_imported + result.personal_skills_imported;
      if (total === 0) {
        setError("Nothing new — no skills found, or they are all here already.");
      } else if (result.personal_skills_imported > 0) {
        setError(
          `Imported ${total}. The ${result.personal_skills_imported} from ~/.claude/skills came in switched off — turn on the ones that apply here.`,
        );
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function save(skill: Skill) {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("save_skill", { skill });
      setDraft(null);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(skillId: string) {
    try {
      await invoke<boolean>("delete_skill", { skillId });
      if (draft?.id === skillId) setDraft(null);
      await load();
    } catch (e) {
      setError(String(e));
    }
  }

  const enabled = skills.filter((s) => s.enabled).length;

  return (
    <div className="flex min-h-0 grow gap-[26px] px-7 pt-[18px] pb-[26px]">
      <div className="flex w-[300px] shrink-0 flex-col gap-3">
        <SectionRule
          label="Skills"
          trailing={skills.length ? `${enabled}/${skills.length} on` : (project ?? undefined)}
        />
        <p className="text-[11px] leading-relaxed text-deck-faint">
          Stored on <span className="text-deck-dim">{project ?? "no project"}</span>. Enabled
          skills go into every agent's system prompt when a run starts.
        </p>

        <div className="flex min-h-0 grow flex-col gap-1 overflow-y-auto">
          {skills.length === 0 && (
            <p className="text-[11.5px] leading-relaxed text-deck-faint">
              Nothing yet. A skill is a procedure you would otherwise repeat in every objective —
              how to write a migration here, what a review must check.
            </p>
          )}
          {skills.map((skill) => (
            <div
              key={skill.id}
              className={cn(
                "flex items-center gap-2 rounded-md px-2 py-1.5 transition-colors",
                draft?.id === skill.id ? "bg-deck-accent-wash" : "hover:bg-deck-surface",
              )}
            >
              {/* The toggle is deliberately not inside the row's own click target: turning a
                  skill off is a different intent from opening it, and conflating them makes one
                  of the two happen by accident. */}
              <input
                type="checkbox"
                checked={skill.enabled}
                onChange={() => void save({ ...skill, enabled: !skill.enabled })}
                title={skill.enabled ? "Sent to every agent" : "Kept, but not sent"}
                className="size-3 shrink-0 accent-[var(--color-deck-accent)]"
              />
              <button
                onClick={() => setDraft(skill)}
                className="flex min-w-0 grow flex-col text-left"
              >
                <span
                  className={cn(
                    "truncate text-[12.5px]",
                    skill.enabled ? "text-deck-text" : "text-deck-faint",
                  )}
                >
                  {skill.name}
                </span>
                {skill.description && (
                  <span className="truncate text-[11px] text-deck-faint">
                    {skill.description}
                  </span>
                )}
              </button>
              <button
                onClick={() => void remove(skill.id)}
                title="Delete this skill"
                className="shrink-0 text-deck-faint transition-colors hover:text-deck-attention"
              >
                <Trash2 className="size-3" />
              </button>
            </div>
          ))}
        </div>

        <div className="flex items-center gap-1">
          <Button variant="ghost" size="sm" onClick={() => setDraft(BLANK)}>
            <Plus /> New skill
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={importFromRepo}
            disabled={busy}
            title="Read this repository's .claude/skills and CLAUDE.md"
          >
            <Download /> Import from repo
          </Button>
        </div>
      </div>

      <div className="flex min-w-0 grow flex-col gap-3">
        {error && (
          <p className="rounded-md border border-deck-attention/30 bg-deck-attention-wash px-3 py-2 text-[11.5px] text-deck-attention">
            {error}
          </p>
        )}
        {draft ? (
          <SkillEditor
            key={draft.id || "new"}
            skill={draft}
            busy={busy}
            onSave={save}
            onCancel={() => setDraft(null)}
          />
        ) : (
          <p className="text-[11.5px] leading-relaxed text-deck-faint">
            Pick a skill to edit, or write a new one. Enabled skills are appended to every agent's
            system prompt when a run starts.
          </p>
        )}
      </div>
    </div>
  );
}

function SkillEditor({
  skill,
  busy,
  onSave,
  onCancel,
}: {
  skill: Skill;
  busy: boolean;
  onSave: (skill: Skill) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState(skill);

  return (
    <div className="flex min-h-0 grow flex-col gap-3">
      <div className="flex items-end gap-2">
        <label className="flex min-w-0 grow flex-col gap-1">
          <span className="label-micro">Name</span>
          <Input
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            placeholder="Writing a migration"
          />
        </label>
        <Button
          variant="primary"
          size="md"
          disabled={busy || !draft.name.trim()}
          onClick={() => onSave(draft)}
        >
          Save
        </Button>
        <Button variant="ghost" size="md" onClick={onCancel}>
          Cancel
        </Button>
      </div>

      <label className="flex flex-col gap-1">
        <span className="label-micro">When to use it</span>
        <Input
          value={draft.description}
          onChange={(e) => setDraft({ ...draft, description: e.target.value })}
          placeholder="Any task that changes the database schema"
        />
      </label>

      <label className="flex min-h-0 grow flex-col gap-1">
        <span className="label-micro">Procedure</span>
        <textarea
          value={draft.body}
          onChange={(e) => setDraft({ ...draft, body: e.target.value })}
          spellCheck={false}
          placeholder={"Write the SQL by hand in crates/deck-core/migrations.\nNumber it sequentially. Never edit a migration that has shipped."}
          className="min-h-0 grow resize-none rounded-[var(--radius-panel)] border border-deck-line bg-deck-surface p-4 font-mono text-[12px] leading-[19px] text-deck-text placeholder:text-deck-faint/70 focus:border-deck-accent focus:outline-none"
        />
      </label>
    </div>
  );
}
