-- V055: secret_rotation — when each operational secret was last rotated.
--
-- Context (2026-08-24): a storage-account key was found sitting in plaintext in
-- a world-readable script on the prod VM, and nobody could say how old it was,
-- because nothing recorded rotations. The five-month backup gap had the same
-- shape: the information needed to notice existed nowhere.
--
-- This table is the ONLY durable record of rotation, written by
-- scripts/rotate-secrets.sh. It deliberately stores no secret material and no
-- hash of any — just the identity of the secret, when it was last rotated, and
-- by whom. The read side (GET /api/admin/secrets) joins it against a static
-- inventory so a secret that has NEVER been rotated still appears, which is the
-- case that matters most.
--
-- One row per secret (latest state), not a history: the panel answers "how old
-- is this?", and the per-rotation audit trail already lands in service_log.

CREATE TABLE IF NOT EXISTS secret_rotation (
    secret_key  VARCHAR(64)  NOT NULL,
    rotated_at  TIMESTAMPTZ  NOT NULL,
    rotated_by  VARCHAR(128) NULL,
    note        TEXT         NULL,
    CONSTRAINT pk_secret_rotation PRIMARY KEY (secret_key)
);
