-- 0006_lifecycle (down)
DELETE FROM configs WHERE key='orphan_retention_days';
ALTER TABLE nodes DROP COLUMN IF EXISTS retired_at;
