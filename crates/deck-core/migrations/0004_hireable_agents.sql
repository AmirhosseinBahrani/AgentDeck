-- Lets the team be hired rather than fixed.
--
-- `role` was constrained to three seeded values. The supervisor already matches tasks to agents
-- by role *string* — the planner is told which roles exist and picks one per task — so an
-- arbitrary role works end to end the moment the constraint allows it. A "Database Engineer"
-- becomes assignable simply by existing.
--
-- `active` is what revoking sets. Deleting the row instead would take every session, report and
-- decision that referenced the agent with it, and the whole point of the run record is that you
-- can still read what happened after someone leaves the team.
PRAGMA foreign_keys = OFF;

CREATE TABLE agents_new (
    id                      TEXT PRIMARY KEY,
    workspace_id            TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    project_id              TEXT REFERENCES projects (id) ON DELETE SET NULL,
    name                    TEXT NOT NULL,
    slug                    TEXT NOT NULL,
    -- Free-form: the role is the capability the supervisor assigns against, not an enum.
    role                    TEXT NOT NULL,
    description             TEXT,
    model_json              TEXT NOT NULL DEFAULT '{}',
    system_prompt           TEXT,
    permission_policy_json  TEXT NOT NULL DEFAULT '{}',
    allowed_tools_json      TEXT NOT NULL DEFAULT '[]',
    disallowed_tools_json   TEXT NOT NULL DEFAULT '[]',
    tools_json              TEXT NOT NULL DEFAULT '[]',
    mcp_server_ids_json     TEXT NOT NULL DEFAULT '[]',
    git_strategy            TEXT NOT NULL DEFAULT 'isolated_worktree'
                              CHECK (git_strategy IN ('isolated_worktree', 'shared_readonly', 'shared_worktree')),
    environment_json        TEXT NOT NULL DEFAULT '{}',
    max_concurrent_sessions INTEGER NOT NULL DEFAULT 1,
    max_budget_usd          REAL,
    is_seeded               INTEGER NOT NULL DEFAULT 0,
    -- 0 once revoked. The agent leaves the roster; its history stays readable.
    active                  INTEGER NOT NULL DEFAULT 1,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    UNIQUE (workspace_id, slug)
);

INSERT INTO agents_new (
    id, workspace_id, project_id, name, slug, role, description, model_json, system_prompt,
    permission_policy_json, allowed_tools_json, disallowed_tools_json, tools_json,
    mcp_server_ids_json, git_strategy, environment_json, max_concurrent_sessions, max_budget_usd,
    is_seeded, created_at, updated_at
)
SELECT
    id, workspace_id, project_id, name, slug, role, description, model_json, system_prompt,
    permission_policy_json, allowed_tools_json, disallowed_tools_json, tools_json,
    mcp_server_ids_json, git_strategy, environment_json, max_concurrent_sessions, max_budget_usd,
    is_seeded, created_at, updated_at
FROM agents;

DROP TABLE agents;
ALTER TABLE agents_new RENAME TO agents;

PRAGMA foreign_keys = ON;
