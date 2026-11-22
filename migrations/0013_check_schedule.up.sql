-- 0013_check_schedule (up) — M3 CheckJob scheduling knobs (ruling 2026-11-19).
-- `node_alive_check_window_minutes` is the HEALTH WINDOW, deliberately NOT reused as
-- the scheduling interval; the schedule gets its own keys with safe defaults.
INSERT INTO configs (key, value) VALUES
    ('check_schedule_interval_minutes', '30'),
    ('check_batch_size', '50')
ON CONFLICT (key) DO NOTHING;