-- 0010_config_wiring (up) — wire the config keys that had no consumer, and add
-- format template storage (B3 gap #3). Decisions: A5 (2026-11-13).
--
-- 1. A5: check history keeps 14 days. Only rewrite an untouched seed value (7), so a
--    deliberate operator setting is never clobbered.
UPDATE configs SET value = '14', updated_at = now()
 WHERE key = 'check_run_history_days' AND value = '7';

-- 2. `score_min_samples` is read at runtime (scoring::params) but was never seeded.
INSERT INTO configs (key, value) VALUES ('score_min_samples', '1')
ON CONFLICT (key) DO NOTHING;

-- 3. Query-plan evidence for the timeline queries
--    (`WHERE node_id=$1 ORDER BY checked_at DESC`): add the composite index.
CREATE INDEX IF NOT EXISTS idx_node_check_results_node_time
    ON node_check_results (node_id, checked_at DESC);

-- 4. Per-format subscription templates (upstream `/api/v1/format-configs` equivalent).
CREATE TABLE IF NOT EXISTS format_configs (
    format     varchar(32) PRIMARY KEY,
    config     jsonb       NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
