-- 0013_check_schedule (down) — remove the scheduling knobs and the push flag.
DELETE FROM configs WHERE key IN ('check_schedule_interval_minutes', 'check_batch_size');
ALTER TABLE check_runs DROP COLUMN IF EXISTS pushed;