-- Standing instructions that outlive a run.
--
-- Agents are spawned with `--setting-sources ''`, which is deliberate: the M0 spike measured a
-- 4x cost increase and a 34-tool surface when a worker inherited the human's settings, and an
-- agent that picks up ambient hooks and MCP servers is not reproducible. But that same flag also
-- means an agent never reads the repository's CLAUDE.md, so there was no way at all to tell the
-- team something true about the project — "migrations are hand-written", "never touch the
-- generated client" — without repeating it in every objective.
--
-- These two tables are that channel. Their contents are rendered into the system prompt at
-- spawn, so what an operator writes here is what the agents are actually told.

-- One note per project. A single document rather than rows because it is prose the operator
-- edits as a whole, and versioning individual paragraphs would imply a structure it does not have.
CREATE TABLE project_memory (
    project_id  TEXT PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    content     TEXT NOT NULL DEFAULT '',
    updated_at  INTEGER NOT NULL
);

-- Named procedures, separate from memory because they are individually switchable.
--
-- A skill that is wrong for the current piece of work should be turned off without deleting what
-- it says, and a skill costs prompt tokens on every single spawn — so `enabled` is the field that
-- makes a large library affordable.
CREATE TABLE project_skills (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    -- When to reach for it. Rendered alongside the body so an agent can tell whether it applies.
    description TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL DEFAULT '',
    enabled     INTEGER NOT NULL DEFAULT 1,
    updated_at  INTEGER NOT NULL,
    UNIQUE (project_id, name)
);

CREATE INDEX idx_project_skills_project ON project_skills (project_id, enabled);
