-- Which model does the work, and which one supervises.
--
-- Both were left unset, so every call took whatever the CLI defaults to. That is a fine default
-- and a poor only-option: the two roles have genuinely different shapes. Workers run long duplex
-- sessions doing the actual engineering, while the supervisor makes short schema-constrained
-- decisions — planning, choosing an assignee, judging a review — where a cheaper model may be
-- entirely adequate, or where a stronger one is worth it precisely because a bad plan wastes
-- every worker's time downstream.
--
-- NULL means "whatever the CLI would pick". Stored rather than defaulted to a pinned id so that
-- a project does not silently keep using a model that has been superseded.
CREATE TABLE project_models (
    project_id       TEXT PRIMARY KEY REFERENCES projects (id) ON DELETE CASCADE,
    worker_model     TEXT,
    supervisor_model TEXT,
    updated_at       INTEGER NOT NULL
);
