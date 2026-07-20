-- V042: re-assert the perf_log DDL on the MAIN database.
--
-- Why this exists: the Rust dashboard's perf_log module created this table in
-- its separate LOGS database (crates/networker-dashboard/src/db/perf_log.rs
-- ensure_schema) while V023 was recorded as applied in the MAIN database's
-- _migrations bookkeeping. On such a database the C# SchemaMigrator skips
-- V023, and the single-DB control plane's /api/perf-log endpoints 500 with
-- 42P01 (relation "perf_log" does not exist) — found live on prod.
--
-- The DDL below is the exact shape of V023_perf_log.sql (same columns, same
-- FK, same indexes) and is fully idempotent: on a fresh database where V023
-- already ran, every statement no-ops. The cumulative schema.sql snapshot is
-- unchanged — this migration adds nothing that a fresh chain does not
-- already build.

CREATE TABLE IF NOT EXISTS perf_log (
    id              BIGSERIAL       PRIMARY KEY,
    logged_at       TIMESTAMPTZ     NOT NULL DEFAULT now(),
    user_id         UUID            REFERENCES dash_user(user_id),
    session_id      VARCHAR(64),
    kind            VARCHAR(10)     NOT NULL,

    -- API fields
    method          VARCHAR(10),
    path            VARCHAR(500),
    status          SMALLINT,
    total_ms        REAL,
    server_ms       REAL,
    network_ms      REAL,
    source          VARCHAR(10),

    -- Render fields
    component       VARCHAR(100),
    trigger         VARCHAR(100),
    render_ms       REAL,
    item_count      INT,

    -- Flexible extras
    meta            JSONB
);

CREATE INDEX IF NOT EXISTS ix_perf_log_logged_at ON perf_log (logged_at DESC);
CREATE INDEX IF NOT EXISTS ix_perf_log_user      ON perf_log (user_id, logged_at DESC);
CREATE INDEX IF NOT EXISTS ix_perf_log_kind      ON perf_log (kind, logged_at DESC);
CREATE INDEX IF NOT EXISTS ix_perf_log_path      ON perf_log (path, logged_at DESC) WHERE kind = 'api';
