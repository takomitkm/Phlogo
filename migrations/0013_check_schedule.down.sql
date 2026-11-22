-- 0013_check_schedule (down) — remove the scheduling knobs.
DELETE FROM configs WHERE key IN ('check_schedule_interval_minutes', 'check_batch_size');