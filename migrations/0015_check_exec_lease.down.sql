-- 0015_check_exec_lease (down) — roll back ONLY what this migration added.
DROP INDEX IF EXISTS idx_check_runs_exec_lease;
ALTER TABLE check_runs
    DROP COLUMN IF EXISTS exec_owner,
    DROP COLUMN IF EXISTS exec_lease_until;