-- 0002_uq_sources (up) — race-safe source dedup by (type, content).
CREATE UNIQUE INDEX IF NOT EXISTS uq_sources_type_content ON sources (type, content);
