CREATE TABLE IF NOT EXISTS cloud_connection (
    connection_id    UUID           NOT NULL PRIMARY KEY,
    name             VARCHAR(200)   NOT NULL,
    provider         VARCHAR(20)    NOT NULL,
    config           JSONB          NOT NULL,
    status           VARCHAR(20)    NOT NULL DEFAULT 'pending',
    last_validated   TIMESTAMPTZ,
    validation_error TEXT,
    created_by       UUID           REFERENCES dash_user(user_id),
    created_at       TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_cloud_connection_provider ON cloud_connection (provider);
