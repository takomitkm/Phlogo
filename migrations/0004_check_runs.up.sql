-- 0004_check_runs (up) — minimal check-run table for P3-01 (idempotency by id).
-- p3-04 will extend persistence (node_check_results / node_status).
CREATE TABLE check_runs (
    id          uuid PRIMARY KEY,
    checker_tag varchar(64) NOT NULL DEFAULT 'default',
    node_count  integer NOT NULL DEFAULT 0,
    status      varchar(16) NOT NULL DEFAULT 'started',
    started_at  timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);
