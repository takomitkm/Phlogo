-- 0001_init (up) — P1 initial schema (sources/nodes/node_sources/source_fetch_runs/configs)
-- Source of truth: plan §5.1 + CONVENTIONS §2.3/§2.6. Do NOT create apalis_* tables here.

CREATE TABLE sources (
    id            uuid PRIMARY KEY,
    type          varchar(32) NOT NULL,
    identifier    text NOT NULL DEFAULT '',
    info          text NOT NULL DEFAULT '',
    content       text NOT NULL,
    status        varchar(16) NOT NULL DEFAULT 'active',
    ignore_dead   boolean NOT NULL DEFAULT false,
    fetch_policy  varchar(16) NOT NULL DEFAULT 'replace',
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_sources_type_status ON sources (type, status);

CREATE TABLE nodes (
    id                  uuid PRIMARY KEY,
    identity_hash       varchar(64) NOT NULL UNIQUE,
    fingerprint_version integer NOT NULL DEFAULT 1,
    protocol            varchar(32) NOT NULL,
    server              varchar(255) NOT NULL,
    port                integer NOT NULL,
    first_seen_at       timestamptz NOT NULL DEFAULT now(),
    last_seen_at        timestamptz NOT NULL DEFAULT now(),
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_nodes_server ON nodes (server);

CREATE TABLE node_sources (
    node_id       uuid NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    source_id     uuid NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    first_seen_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (node_id, source_id)
);

CREATE TABLE source_fetch_runs (
    id          uuid PRIMARY KEY,
    source_id   uuid NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    status      varchar(16) NOT NULL,
    started_at  timestamptz NOT NULL,
    finished_at timestamptz,
    counts      jsonb NOT NULL DEFAULT '{}'::jsonb,
    error       text,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_fetch_runs_source ON source_fetch_runs (source_id, started_at DESC);

CREATE TABLE configs (
    key        varchar(128) PRIMARY KEY,
    value      text NOT NULL DEFAULT '',
    updated_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO configs (key, value) VALUES
    ('max_distribute',                  '200'),
    ('dead_latency_ms',                 '3000'),
    ('fetch_dead_window_days',          '7'),
    ('node_dead_window_days',           '4'),
    ('fetch_history_window_days',       '7'),
    ('score_decay_alpha',               '0.7'),
    ('score_weight_latency',            '0.7'),
    ('score_weight_stability',          '0.3'),
    ('score_weight_speed',              '0.55'),
    ('score_latency_curve_k',           '0.3'),
    ('score_jitter_linear_threshold_ms','200'),
    ('grade_a_min',                     '0.75'),
    ('grade_b_min',                     '0.50'),
    ('grade_c_min',                     '0.25'),
    ('node_alive_check_window_minutes', '240'),
    ('check_run_history_days',          '7'),
    ('cleanup_interval_minutes',        '30'),
    ('default_sponsor_text',            ''),
    ('node_distribute_filter',          ''),
    ('node_name_template',              '(unlock_flags != "" ? "[" + unlock_flags + "] " : "") + node.grade + " - " + node.region_flag + " " + node.region + " " + node.seq + (node.sponsor != "" ? " " + node.sponsor : "")'),
    ('filename_clash',                  ''),
    ('filename_singbox',                ''),
    ('filename_base64',                 '')
ON CONFLICT (key) DO NOTHING;
