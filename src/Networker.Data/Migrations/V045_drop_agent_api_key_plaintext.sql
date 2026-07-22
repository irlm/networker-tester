-- V045: drop the plaintext agent.api_key column.
--
-- Since V040 agent auth has looked up keys ONLY by api_key_hash (lowercase-hex
-- SHA-256, constant-time compared — AgentMessageProcessor.AuthenticateAsync);
-- the plaintext api_key column has been write-only dead weight ever since
-- (minting + rotation write both columns, but nothing reads the plaintext one:
-- it is never serialized, and rotation returns the freshly-generated key
-- directly rather than re-reading the row). A secrets audit (2026-07,
-- docs/analysis/secrets-audit-2026-07.md) flagged the lingering plaintext column
-- as pure liability, so this removes it.
--
-- Order matters: the UNIQUE index agent_api_key_key is defined ON the plaintext
-- column, so it must be dropped first (api_key_hash keeps its own unique index
-- agent_api_key_hash_key). Idempotent (IF EXISTS) so re-running the chain no-ops
-- and a database already lacking the column/index is unaffected.
DROP INDEX IF EXISTS agent_api_key_key;

ALTER TABLE agent DROP COLUMN IF EXISTS api_key;
