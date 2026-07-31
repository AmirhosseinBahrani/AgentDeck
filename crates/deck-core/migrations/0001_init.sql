-- AgentDeck initial schema.
--
-- Conventions: ids are TEXT uuids, timestamps are INTEGER ms since epoch, and structured
-- payloads live in `*_json TEXT`. `events` is the append-only spine; `messages` and
-- `tool_calls` are projections off it kept for fast UI reads.

PRAGMA foreign_keys = ON;

CREATE TABLE workspaces (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    settings_json TEXT NOT NULL DEFAULT '{}',
    created_at    INTEGER NOT NULL
);

CREATE TABLE projects (
    id             TEXT PRIMARY KEY,
    workspace_id   TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    path           TEXT NOT NULL,
    repository     TEXT,
    default_branch TEXT NOT NULL DEFAULT 'main',
    policy_json    TEXT NOT NULL DEFAULT '{}',
    created_at     INTEGER NOT NULL,
    UNIQUE (workspace_id, path)
);

CREATE TABLE agents (
    id                      TEXT PRIMARY KEY,
    workspace_id            TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    project_id              TEXT REFERENCES projects (id) ON DELETE SET NULL,
    name                    TEXT NOT NULL,
    slug                    TEXT NOT NULL,
    role                    TEXT NOT NULL CHECK (role IN ('supervisor', 'developer', 'reviewer')),
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
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    UNIQUE (workspace_id, slug)
);

CREATE TABLE supervisor_runs (
    id           TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    objective    TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN (
                     'planning', 'dispatching', 'monitoring', 'reviewing', 'replanning',
                     'blocked_on_human', 'completed', 'failed', 'cancelled')),
    autonomy     TEXT NOT NULL DEFAULT 'assisted'
                   CHECK (autonomy IN ('manual', 'assisted', 'autonomous')),
    iteration    INTEGER NOT NULL DEFAULT 0,
    budget_usd   REAL,
    -- Accumulated by SUMMING per-turn result costs; the CLI reports cost per turn, not
    -- cumulatively, so replacing this value instead of adding would undercount badly.
    spent_usd    REAL NOT NULL DEFAULT 0,
    started_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    ended_at     INTEGER
);

CREATE INDEX idx_runs_active ON supervisor_runs (project_id, status);

CREATE TABLE worktrees (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    agent_id   TEXT REFERENCES agents (id) ON DELETE SET NULL,
    task_id    TEXT,
    path       TEXT NOT NULL UNIQUE,
    branch     TEXT NOT NULL,
    base_ref   TEXT NOT NULL,
    -- Pinned at creation. A branch name would move under us and make a review diff
    -- irreproducible once main advances.
    base_sha   TEXT NOT NULL,
    status     TEXT NOT NULL CHECK (status IN ('active', 'dirty', 'merged', 'abandoned', 'removed')),
    created_at INTEGER NOT NULL,
    removed_at INTEGER
);

CREATE TABLE tasks (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    supervisor_run_id TEXT REFERENCES supervisor_runs (id) ON DELETE SET NULL,
    parent_task_id    TEXT REFERENCES tasks (id) ON DELETE SET NULL,
    title             TEXT NOT NULL,
    description       TEXT NOT NULL DEFAULT '',
    status            TEXT NOT NULL CHECK (status IN (
                          'backlog', 'queued', 'assigned', 'running', 'blocked',
                          'review', 'completed', 'failed', 'cancelled')),
    priority          INTEGER NOT NULL DEFAULT 100,
    assignee_agent_id TEXT REFERENCES agents (id) ON DELETE SET NULL,
    -- acceptance_criteria / constraints / deliverables. Validated so that every code task
    -- carries at least one executable verification.
    contract_json     TEXT NOT NULL DEFAULT '{}',
    contract_version  INTEGER NOT NULL DEFAULT 1,
    -- Vec<ResourceKey> the scheduler must lock before dispatch.
    resources_json    TEXT NOT NULL DEFAULT '[]',
    -- Marks a task the objective cannot be considered complete without.
    objective_gate    INTEGER NOT NULL DEFAULT 0,
    attempts          INTEGER NOT NULL DEFAULT 0,
    max_attempts      INTEGER NOT NULL DEFAULT 3,
    review_rounds     INTEGER NOT NULL DEFAULT 0,
    fix_chain_depth   INTEGER NOT NULL DEFAULT 0,
    retry_policy_json TEXT NOT NULL DEFAULT '{}',
    failure_reason    TEXT,
    review_status     TEXT,
    blocking_task_id  TEXT REFERENCES tasks (id) ON DELETE SET NULL,
    branch            TEXT,
    worktree_id       TEXT REFERENCES worktrees (id) ON DELETE SET NULL,
    created_at        INTEGER NOT NULL,
    started_at        INTEGER,
    completed_at      INTEGER
);

CREATE INDEX idx_tasks_ready    ON tasks (project_id, status, priority DESC, created_at);
CREATE INDEX idx_tasks_assignee ON tasks (assignee_agent_id, status);
CREATE INDEX idx_tasks_run      ON tasks (supervisor_run_id, status);

CREATE TABLE task_dependencies (
    task_id            TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    depends_on_task_id TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    -- Soft edges order work without blocking readiness.
    hardness           TEXT NOT NULL DEFAULT 'hard' CHECK (hardness IN ('hard', 'soft')),
    PRIMARY KEY (task_id, depends_on_task_id),
    CHECK (task_id <> depends_on_task_id)
);

CREATE INDEX idx_deps_rev ON task_dependencies (depends_on_task_id);

CREATE TABLE sessions (
    id              TEXT PRIMARY KEY, -- the uuid we pass as --session-id
    agent_id        TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    project_id      TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    task_id         TEXT REFERENCES tasks (id) ON DELETE SET NULL,
    worktree_id     TEXT REFERENCES worktrees (id) ON DELETE SET NULL,
    status          TEXT NOT NULL CHECK (status IN (
                        'starting', 'running', 'idle', 'interrupted', 'stopped', 'crashed', 'failed')),
    resumed_from    TEXT REFERENCES sessions (id),
    -- Claude buckets transcripts by working directory, so --resume only finds a
    -- conversation when re-invoked from this exact path. Persisted for that reason.
    cwd             TEXT NOT NULL,
    cli_version     TEXT,
    model           TEXT,
    permission_mode TEXT,
    argv_json       TEXT NOT NULL DEFAULT '[]',
    cost_usd        REAL NOT NULL DEFAULT 0,
    num_turns       INTEGER NOT NULL DEFAULT 0,
    exit_code       INTEGER,
    exit_reason     TEXT,
    stderr_tail     TEXT,
    started_at      INTEGER,
    last_activity_at INTEGER,
    ended_at        INTEGER
);

CREATE INDEX idx_sessions_agent ON sessions (agent_id, started_at DESC);
CREATE INDEX idx_sessions_live  ON sessions (status)
    WHERE status IN ('starting', 'running', 'idle');

CREATE TABLE events (
    seq               INTEGER PRIMARY KEY AUTOINCREMENT,
    at                INTEGER NOT NULL,
    kind              TEXT NOT NULL,
    session_id        TEXT,
    agent_id          TEXT,
    task_id           TEXT,
    supervisor_run_id TEXT,
    payload_json      TEXT NOT NULL
);

CREATE INDEX idx_events_session ON events (session_id, seq);
CREATE INDEX idx_events_task    ON events (task_id, seq);
CREATE INDEX idx_events_kind    ON events (kind, seq);

CREATE TABLE messages (
    id                 TEXT PRIMARY KEY,
    session_id         TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    seq                INTEGER NOT NULL,
    role               TEXT NOT NULL,
    content_json       TEXT NOT NULL,
    parent_tool_use_id TEXT,
    at                 INTEGER NOT NULL
);

CREATE INDEX idx_messages_session ON messages (session_id, seq);

CREATE TABLE tool_calls (
    id          TEXT PRIMARY KEY, -- the Anthropic tool_use_id
    session_id  TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    task_id     TEXT,
    tool_name   TEXT NOT NULL,
    input_json  TEXT NOT NULL,
    output_json TEXT,
    is_error    INTEGER,
    decision    TEXT CHECK (decision IS NULL OR decision IN ('allow', 'deny', 'ask')),
    decided_by  TEXT CHECK (decided_by IS NULL OR decided_by IN ('policy', 'human', 'timeout')),
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER
);

CREATE INDEX idx_tools_session ON tool_calls (session_id, started_at);

CREATE TABLE permission_requests (
    id                  TEXT PRIMARY KEY, -- the CLI's control request_id
    session_id          TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    agent_id            TEXT NOT NULL,
    task_id             TEXT,
    tool_name           TEXT NOT NULL,
    input_json          TEXT NOT NULL,
    -- The CLI classifies why it is asking (e.g. 'workingDir') and which path was blocked,
    -- so policy applies to a pre-classified decision rather than re-deriving containment.
    decision_reason     TEXT,
    reason_type         TEXT,
    blocked_path        TEXT,
    -- Structured addRules/addDirectories options, surfaced to the UI as typed choices.
    suggestions_json    TEXT NOT NULL DEFAULT '[]',
    status              TEXT NOT NULL CHECK (status IN (
                            'pending', 'allowed', 'denied', 'timeout', 'superseded')),
    decision_json       TEXT,
    resolved_by         TEXT,
    created_at          INTEGER NOT NULL,
    resolved_at         INTEGER
);

CREATE INDEX idx_perm_pending ON permission_requests (status, created_at)
    WHERE status = 'pending';

CREATE TABLE agent_runtime_states (
    agent_id           TEXT PRIMARY KEY REFERENCES agents (id) ON DELETE CASCADE,
    state              TEXT NOT NULL CHECK (state IN (
                           'offline', 'starting', 'idle', 'working', 'waiting',
                           'blocked', 'awaiting_review', 'error')),
    current_task_id    TEXT REFERENCES tasks (id) ON DELETE SET NULL,
    current_session_id TEXT REFERENCES sessions (id) ON DELETE SET NULL,
    current_activity   TEXT,
    last_report_at     INTEGER,
    updated_at         INTEGER NOT NULL
);

CREATE TABLE agent_reports (
    id                      TEXT PRIMARY KEY,
    agent_id                TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    task_id                 TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    session_id              TEXT,
    status                  TEXT NOT NULL CHECK (status IN ('working', 'blocked', 'completed', 'failed')),
    summary                 TEXT NOT NULL,
    progress                REAL,
    completed_work_json     TEXT NOT NULL DEFAULT '[]',
    remaining_work_json     TEXT NOT NULL DEFAULT '[]',
    blockers_json           TEXT NOT NULL DEFAULT '[]',
    changed_files_json      TEXT NOT NULL DEFAULT '[]',
    tests_run_json          TEXT NOT NULL DEFAULT '[]',
    needs_supervisor_action INTEGER NOT NULL DEFAULT 0,
    created_at              INTEGER NOT NULL
);

CREATE INDEX idx_reports_task ON agent_reports (task_id, created_at DESC);

CREATE TABLE reviews (
    id                TEXT PRIMARY KEY,
    task_id           TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    reviewer_agent_id TEXT REFERENCES agents (id) ON DELETE SET NULL,
    verdict           TEXT CHECK (verdict IS NULL OR verdict IN (
                          'pass', 'pass_with_notes', 'fail', 'inconclusive')),
    -- Captured exit codes from verification commands the supervisor ran itself. The
    -- reviewer never gets to override a red test.
    verification_json TEXT NOT NULL DEFAULT '[]',
    findings_json     TEXT NOT NULL DEFAULT '[]',
    diff_base_sha     TEXT,
    diff_head_sha     TEXT,
    created_at        INTEGER NOT NULL
);

CREATE INDEX idx_reviews_task ON reviews (task_id, created_at DESC);

CREATE TABLE escalations (
    id           TEXT PRIMARY KEY,
    supervisor_run_id TEXT REFERENCES supervisor_runs (id) ON DELETE CASCADE,
    task_id      TEXT REFERENCES tasks (id) ON DELETE CASCADE,
    agent_id     TEXT REFERENCES agents (id) ON DELETE SET NULL,
    kind         TEXT NOT NULL,
    -- Task scope freezes only that task and its dependents; other agents keep working.
    scope        TEXT NOT NULL DEFAULT 'task' CHECK (scope IN ('task', 'subgraph', 'run')),
    question     TEXT NOT NULL,
    options_json TEXT NOT NULL DEFAULT '[]',
    context_json TEXT NOT NULL DEFAULT '{}',
    -- Deduplicates repeat escalations for the same underlying condition.
    fingerprint  TEXT,
    status       TEXT NOT NULL CHECK (status IN ('open', 'answered', 'expired', 'superseded')),
    answer_json  TEXT,
    created_at   INTEGER NOT NULL,
    answered_at  INTEGER
);

CREATE INDEX idx_escalations_open ON escalations (status, created_at) WHERE status = 'open';

CREATE TABLE decisions (
    id                  TEXT PRIMARY KEY,
    supervisor_run_id   TEXT NOT NULL REFERENCES supervisor_runs (id) ON DELETE CASCADE,
    iteration           INTEGER NOT NULL,
    at                  INTEGER NOT NULL,
    stage               TEXT NOT NULL,
    kind                TEXT NOT NULL,
    decided_by          TEXT NOT NULL CHECK (decided_by IN ('code', 'claude', 'human')),
    rule_id             TEXT,
    -- Written before the LLM call so a crash mid-decision is recoverable, and so the log
    -- can be replayed against the pure stages as a regression corpus.
    status              TEXT NOT NULL CHECK (status IN ('requested', 'validated', 'rejected', 'applied')),
    inputs_digest       TEXT,
    inputs_json         TEXT,
    raw_output_json     TEXT,
    validated_output_json TEXT,
    validation_errors   TEXT,
    repair_count        INTEGER NOT NULL DEFAULT 0,
    rationale           TEXT,
    effects_json        TEXT NOT NULL DEFAULT '[]',
    cost_usd            REAL,
    latency_ms          INTEGER,
    model               TEXT,
    parent_decision_id  TEXT REFERENCES decisions (id) ON DELETE SET NULL
);

CREATE INDEX idx_decisions_run ON decisions (supervisor_run_id, iteration, at);

-- Idempotency for the supervisor loop: written in the same transaction as the stage's
-- mutations, so a restart skips stages that already committed.
CREATE TABLE stage_receipts (
    supervisor_run_id TEXT NOT NULL REFERENCES supervisor_runs (id) ON DELETE CASCADE,
    iteration         INTEGER NOT NULL,
    stage             TEXT NOT NULL,
    completed_at      INTEGER NOT NULL,
    PRIMARY KEY (supervisor_run_id, iteration, stage)
);

-- Intent-before-effect for non-DB side effects (spawn, worktree create, branch create).
-- reconcile() replays these against reality on boot.
CREATE TABLE intents (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status      TEXT NOT NULL CHECK (status IN ('pending', 'applied', 'abandoned')),
    created_at  INTEGER NOT NULL,
    settled_at  INTEGER
);

CREATE INDEX idx_intents_pending ON intents (status, created_at) WHERE status = 'pending';

CREATE TABLE resource_locks (
    resource_key TEXT NOT NULL,
    mode         TEXT NOT NULL CHECK (mode IN ('shared', 'exclusive')),
    task_id      TEXT NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    acquired_at  INTEGER NOT NULL,
    PRIMARY KEY (resource_key, task_id)
);

-- Lets a boot-time reaper kill process trees orphaned by an app crash. start time is
-- stored alongside pid so pid reuse cannot cause us to signal an unrelated process.
CREATE TABLE runtime_processes (
    session_id     TEXT PRIMARY KEY,
    pid            INTEGER NOT NULL,
    pgid           INTEGER NOT NULL,
    started_at_ms  INTEGER NOT NULL,
    worktree_path  TEXT
);

CREATE TABLE mcp_servers (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    transport    TEXT NOT NULL,
    config_json  TEXT NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1,
    UNIQUE (workspace_id, name)
);

-- Rolling rate-limit state derived from the CLI's rate_limit_event. Global, not
-- per-session: throttling is an account-wide condition, so backoff must be too.
CREATE TABLE rate_limit_state (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    status        TEXT NOT NULL,
    limit_type    TEXT,
    resets_at     INTEGER,
    observed_at   INTEGER NOT NULL
);
