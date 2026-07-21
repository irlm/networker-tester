-- V044: agent api_key lifecycle columns — expiry, last-used tracking.
--
-- Agent api-key hardening wave. The 48-char CSPRNG agent api_key (hashed at
-- rest since V040) gains:
--   * api_key_expires_at   — when non-null and in the past, agent auth
--                            (/ws/agent?key= and /hub/agent?key=) rejects the
--                            key with 401 pre-upgrade. NULL = no expiry, so
--                            every fielded agent keeps authenticating unchanged.
--   * api_key_last_used_at  — stamped on each successful auth (write-throttled
--                            to ~once per 5 min per agent), the "last seen"
--                            signal the UI surfaces.
--   * api_key_last_used_ip  — remote IP of that last successful auth (audit).
--
-- The rotate endpoint (POST /api/projects/{projectId}/testers/{testerId}/
-- rotate-key) replaces api_key + api_key_hash and resets api_key_expires_at.
--
-- All three columns are nullable (no default) so the migration cannot fail an
-- in-flight writer and every existing agent is unaffected. Fully idempotent
-- (ADD COLUMN IF NOT EXISTS) so re-running the chain no-ops.
ALTER TABLE agent
    ADD COLUMN IF NOT EXISTS api_key_expires_at   TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS api_key_last_used_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS api_key_last_used_ip VARCHAR(64);
