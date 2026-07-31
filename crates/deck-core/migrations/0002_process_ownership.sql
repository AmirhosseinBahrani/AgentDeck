-- Records which app launch owns each running agent process.
--
-- Without this, a launching app cannot tell its own agents from a *concurrently running*
-- second instance's, and reaping "everything left over" would kill live work belonging to
-- someone else. Ownership is what makes orphan detection safe rather than merely plausible.
ALTER TABLE runtime_processes ADD COLUMN app_boot_id TEXT NOT NULL DEFAULT '';
ALTER TABLE runtime_processes ADD COLUMN app_pid INTEGER NOT NULL DEFAULT 0;
ALTER TABLE runtime_processes ADD COLUMN task_id TEXT;
