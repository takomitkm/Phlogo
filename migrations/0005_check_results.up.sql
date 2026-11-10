-- 0005_check_results (up) — p3-04 persistence tables.
-- check_runs already exists (0004); align columns with plan §5.1.
ALTER TABLE check_runs ADD COLUMN IF NOT EXISTS external_job_id text;

CREATE TABLE node_check_results (
    id            bigserial PRIMARY KEY,
    node_id       uuid NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    check_run_id  uuid NOT NULL REFERENCES check_runs (id) ON DELETE CASCADE,
    checker_tag   varchar(64) NOT NULL,
    metrics       jsonb NOT NULL DEFAULT '{}'::jsonb,
    result_status varchar(16) NOT NULL,
    checked_at    timestamptz NOT NULL,
    UNIQUE (node_id, check_run_id, checker_tag)
);
CREATE INDEX idx_node_check_results_run ON node_check_results (check_run_id);

CREATE TABLE node_status (
    node_id         uuid NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    checker_tag     varchar(64) NOT NULL,
    health          varchar(16) NOT NULL,
    last_checked_at timestamptz NOT NULL,
    PRIMARY KEY (node_id, checker_tag)
);
