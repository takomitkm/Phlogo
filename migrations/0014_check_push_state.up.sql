-- 0014_check_push_state (up) — M3.1-2 delivery state machine (ruling 2026-11-20).
--
-- Replaces the one-shot `pushed boolean` idea (rejected in review: a boolean cannot
-- distinguish "not claimed / dispatching / enqueued", and its claim was race-prone).
--
-- States (`push_state`):
--   'pending'     : run row created, nobody holds a delivery lease yet
--   'dispatching' : an instance holds the delivery lease (lease_until in the future)
--   'enqueued'    : the Apalis job was successfully pushed (terminal for delivery)
--
-- Recovery rule (at-least-once): a row stuck in 'dispatching' whose lease has expired
-- may be re-claimed by any scheduler — the push may or may not have happened before
-- the crash, so at-least-once delivery is accepted BY DESIGN; duplicate MiaoSpeed
-- executions of the same run are prevented on the EXECUTION side by
-- record_check_run's INSERT ... ON CONFLICT DO NOTHING (see check_job.rs) — the first
-- delivery to reach the worker wins the run row and executes the real check, later
-- deliveries of the same run_id see the row already existing and are recorded but the
-- run semantics stay one-shot per run_id.
ALTER TABLE check_runs
    ADD COLUMN IF NOT EXISTS push_state varchar(16) NOT NULL DEFAULT 'pending',
    ADD COLUMN IF NOT EXISTS lease_until timestamptz,
    ADD COLUMN IF NOT EXISTS lease_owner uuid,
    DROP COLUMN IF EXISTS pushed;

-- Lease lookups (recovery scan: expired dispatching rows).
CREATE INDEX IF NOT EXISTS idx_check_runs_push_recovery
    ON check_runs (push_state, lease_until)
    WHERE push_state = 'dispatching';