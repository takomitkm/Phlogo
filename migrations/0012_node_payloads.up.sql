-- 0012_node_payloads (up) — encrypted node payload at rest (M1; plan §7 / §16.8, ruling A2).
--
-- The raw vendor payload is Clash-shaped JSON carrying the node's credentials, so it
-- must not live in plaintext. It is now sealed with XChaCha20-Poly1305 (AAD = node id)
-- by the application and stored here instead of in `nodes.raw_config_reference`.
CREATE TABLE node_payloads (
    node_id     uuid PRIMARY KEY REFERENCES nodes (id) ON DELETE CASCADE,
    key_version integer NOT NULL DEFAULT 1,
    nonce       bytea   NOT NULL CHECK (octet_length(nonce) = 24),
    ciphertext  bytea   NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);

-- Purge credentials written before encryption existed. SQL has no key, so they cannot
-- be re-sealed here: the next fetch of each source repopulates the encrypted payload.
UPDATE nodes SET raw_config_reference = '' WHERE raw_config_reference <> '';