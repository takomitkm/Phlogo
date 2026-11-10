-- 0008_node_score (up) — nodes.score aggregate column (P4-03).
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS score double precision NOT NULL DEFAULT 0;
