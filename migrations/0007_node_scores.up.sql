-- 0007_node_scores (up) — P4-03 scoring persistence.
CREATE TABLE node_scores (
    node_id       uuid NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    checker_tag   varchar(64) NOT NULL,
    score         double precision NOT NULL DEFAULT 0,
    components    jsonb NOT NULL DEFAULT '{}'::jsonb,
    version       varchar(16) NOT NULL DEFAULT 'ewma-v1',
    calculated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (node_id, checker_tag)
);

INSERT INTO configs (key, value) VALUES
    ('score_min_samples', '1')
ON CONFLICT (key) DO NOTHING;
