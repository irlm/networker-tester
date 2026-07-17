CREATE TABLE IF NOT EXISTS system_health (
    id            BIGSERIAL     NOT NULL PRIMARY KEY,
    checked_at    TIMESTAMPTZ   NOT NULL DEFAULT now(),
    check_name    VARCHAR(50)   NOT NULL,
    status        VARCHAR(10)   NOT NULL,
    value         TEXT,
    message       TEXT,
    details       JSONB
);
CREATE INDEX IF NOT EXISTS ix_system_health_checked_at ON system_health(checked_at DESC);
CREATE INDEX IF NOT EXISTS ix_system_health_name ON system_health(check_name, checked_at DESC);
