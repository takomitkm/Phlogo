-- 0013_check_schedule (up) — M3 CheckJob scheduling knobs (ruling 2026-11-19).
-- `node_alive_check_window_minutes` is the HEALTH WINDOW, deliberately NOT reused as
-- the scheduling interval; the schedule gets its own keys with safe defaults.
INSERT INTO configs (key, value) VALUES
    ('check_schedule_interval_minutes', '30'),
    ('check_batch_size', '50')
ON CONFLICT (key) DO NOTHING;

-- M3.1 (ruling 2026-11-20): recoverable schedule→push window. `pushed=false` with
-- status='started' means the run row was claimed but the Apalis job was never pushed
-- (scheduler died in between); a later pass re-pushes exactly once.
ALTER TABLE check_runs ADD COLUMN IF NOT EXISTS pushed boolean NOT NULL DEFAULT false;