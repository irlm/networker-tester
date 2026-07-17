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
