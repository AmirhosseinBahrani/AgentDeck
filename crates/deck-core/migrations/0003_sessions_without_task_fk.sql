-- Lets a session be recorded before its task has been persisted.
--
-- A session is an operating-system process that exists whether or not the supervisor's task
-- graph has been written down; the graph is still held in memory and saved later. With the
-- foreign key in place, recording a real running agent failed outright — which meant the one
-- record needed to resume it, or to kill it after a crash, was the record we could not write.
--
-- `task_id` stays as attribution. SQLite cannot drop a constraint in place, so the table is
-- rebuilt; this is the documented approach and the data is copied column for column.
PRAGMA foreign_keys = OFF;

CREATE TABLE sessions_new (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL REFERENCES agents (id) ON DELETE CASCADE,
    project_id      TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    task_id         TEXT,
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

INSERT INTO sessions_new SELECT * FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE INDEX idx_sessions_agent ON sessions (agent_id, started_at DESC);
CREATE INDEX idx_sessions_live  ON sessions (status)
    WHERE status IN ('starting', 'running', 'idle');

PRAGMA foreign_keys = ON;
