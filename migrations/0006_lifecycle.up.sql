-- 0006_lifecycle (up) — P4 lifecycle support.
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS retired_at timestamptz;

INSERT INTO configs (key, value) VALUES
    ('orphan_retention_days', '14')
ON CONFLICT (key) DO NOTHING;
