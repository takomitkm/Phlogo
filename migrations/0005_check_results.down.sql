-- 0005_check_results (down)
DROP TABLE IF EXISTS node_status;
DROP TABLE IF EXISTS node_check_results;
ALTER TABLE check_runs DROP COLUMN IF EXISTS external_job_id;
