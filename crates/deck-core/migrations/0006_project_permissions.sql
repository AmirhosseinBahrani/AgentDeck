-- How much the team may do without being asked, per project.
--
-- The level was a constant in code: every agent on every repository got `acceptEdits` and the
-- same curated shell allowlist. That is a reasonable middle, and wrong at both ends — a
-- repository someone is still deciding whether to trust the team with wants every write to be a
-- question, and one that is entirely disposable does not want to be asked about `mkdir`.
--
-- Stored per project because trust is a property of the codebase, not of the person. Deny rules
-- are deliberately absent from this table: deny is unioned across policy layers and cannot be
-- overridden downstream, so a stored value that appeared to relax it would be a setting the
-- resolver ignores.
CREATE TABLE project_permissions (
    project_id      TEXT PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    level           TEXT NOT NULL DEFAULT 'standard'
                      CHECK (level IN ('cautious', 'standard', 'trusted')),
    -- Extra shell prefixes auto-approved on top of the level's own list, one per JSON entry.
    extra_bash_json TEXT NOT NULL DEFAULT '[]',
    updated_at      INTEGER NOT NULL
);
