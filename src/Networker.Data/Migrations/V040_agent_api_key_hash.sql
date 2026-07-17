-- V040: hash the agent api_key at rest.
--
-- Agent auth (/ws/agent?key= and /hub/agent?key=) now resolves agents by
-- api_key_hash = lowercase-hex SHA-256 of the plaintext key and verifies it
-- with a constant-time compare; the plaintext column is no longer consulted
-- by the lookup.
--
-- The plaintext api_key column is deliberately KEPT for now: fielded agents
-- still present the plaintext key on connect (zero wire-protocol change) and
-- key minting writes both columns. api_key is dropped by a later migration
-- once the fleet is verified authenticating against the hash.
ALTER TABLE agent
    ADD COLUMN IF NOT EXISTS api_key_hash VARCHAR(64);

-- Backfill existing agents from the plaintext keys already at rest.
-- sha256() and convert_to() are PostgreSQL built-ins (PG11+); no pgcrypto.
UPDATE agent
    SET api_key_hash = encode(sha256(convert_to(api_key, 'UTF8')), 'hex')
    WHERE api_key_hash IS NULL;

-- Same uniqueness guarantee the plaintext column carries. Left nullable so a
-- not-yet-upgraded writer can't hard-fail the insert; every post-V040 mint
-- writes both columns, and auth simply never matches a NULL hash.
CREATE UNIQUE INDEX IF NOT EXISTS agent_api_key_hash_key ON agent (api_key_hash);
