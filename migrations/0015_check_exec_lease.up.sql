-- 0015_check_exec_lease (up) — M3.1-3 execution-side mutual exclusion (ruling 2026-11-22).
--
-- Why a second lease: the DELIVERY lease (0014) decides who may push a job into the
-- queue. It cannot stop two *deliveries* of the same run_id from being consumed by
-- two workers at once — the previous execution gate was a plain `SELECT status`, i.e.
-- a read, so both readers saw 'started' and both called MiaoSpeed. Execution right
-- needs its own atomic claim.
--
--   exec_owner       : worker that currently owns the execution right (uuid)
--   exec_lease_until : when that right expires; an expired lease may be re-claimed
--                      (the owner died mid-check)
--
-- Invariants:
--   * claim is a single UPDATE ... WHERE (lease free or expired) AND status IN
--     ('started','failed') RETURNING — exactly one concurrent caller can win.
--   * the terminal transition (success via persist, failed via fail) clears the lease
--     in the SAME statement, so a finished run never keeps a stale lease.
--   * a run that already reached `success` is not claimable at all.
ALTER TABLE check_runs
    ADD COLUMN IF NOT EXISTS exec_owner uuid,
    ADD COLUMN IF NOT EXISTS exec_lease_until timestamptz;

-- Recovery scans ("which runs are stuck mid-execution?") read this partial index.
CREATE INDEX IF NOT EXISTS idx_check_runs_exec_lease
    ON check_runs (exec_lease_until)
    WHERE exec_lease_until IS NOT NULL;