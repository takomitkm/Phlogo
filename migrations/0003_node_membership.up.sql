-- 0003_node_membership (up) — lifecycle states for P2-04 reconciliation.
-- source_fetch_runs and sources.fetch_policy already exist since 0001.
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS membership varchar(16) NOT NULL DEFAULT 'Linked';
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS orphaned_at timestamptz;
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS raw_config_reference text NOT NULL DEFAULT '';
