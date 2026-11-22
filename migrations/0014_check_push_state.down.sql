-- 0014_check_push_state (down) — roll back ONLY what this migration added.
DROP INDEX IF EXISTS idx_check_runs_push_recovery;
ALTER TABLE check_runs
    DROP COLUMN IF EXISTS push_state,
    DROP COLUMN IF EXISTS lease_until,
    DROP COLUMN IF EXISTS lease_owner;