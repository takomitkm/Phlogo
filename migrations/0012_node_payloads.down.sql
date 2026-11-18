-- 0012_node_payloads (down) — drop the encrypted payload store.
--
-- NOTE: the purge of plaintext `raw_config_reference` is **not reversible**; the
-- original payloads are only recoverable by re-fetching their sources.
DROP TABLE IF EXISTS node_payloads;