-- 0010_config_wiring (down)
DROP TABLE IF EXISTS format_configs;
DROP INDEX IF EXISTS idx_node_check_results_node_time;
DELETE FROM configs WHERE key = 'score_min_samples';
UPDATE configs SET value = '7', updated_at = now()
 WHERE key = 'check_run_history_days' AND value = '14';
