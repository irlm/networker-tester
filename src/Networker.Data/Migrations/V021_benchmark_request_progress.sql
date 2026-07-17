CREATE TABLE IF NOT EXISTS benchmark_request_progress (
    id              BIGSERIAL       PRIMARY KEY,
    config_id       UUID            NOT NULL,
    testbed_id      UUID,
    language        TEXT            NOT NULL,
    mode            TEXT            NOT NULL,
    request_index   INT             NOT NULL,
    total_requests  INT             NOT NULL,
    latency_ms      DOUBLE PRECISION NOT NULL,
    success         BOOLEAN         NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ     NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ix_brp_config_lang
    ON benchmark_request_progress (config_id, language, mode);

CREATE INDEX IF NOT EXISTS ix_brp_config
    ON benchmark_request_progress (config_id);
