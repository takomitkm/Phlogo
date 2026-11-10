-- 0003_node_membership (down)
ALTER TABLE nodes DROP COLUMN IF EXISTS raw_config_reference;
ALTER TABLE nodes DROP COLUMN IF EXISTS orphaned_at;
ALTER TABLE nodes DROP COLUMN IF EXISTS membership;
