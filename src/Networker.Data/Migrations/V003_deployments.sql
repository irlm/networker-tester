CREATE TABLE IF NOT EXISTS deployment (
    deployment_id  UUID           NOT NULL PRIMARY KEY,
    name           VARCHAR(200)   NOT NULL,
    status         VARCHAR(20)    NOT NULL DEFAULT 'pending',
    config         JSONB          NOT NULL,
    provider_summary TEXT,
    created_by     UUID           REFERENCES dash_user(user_id),
    created_at     TIMESTAMPTZ    NOT NULL DEFAULT now(),
    started_at     TIMESTAMPTZ,
    finished_at    TIMESTAMPTZ,
    endpoint_ips   JSONB,
    agent_id       UUID           REFERENCES agent(agent_id),
    error_message  TEXT,
    log            TEXT
);

CREATE INDEX IF NOT EXISTS ix_deployment_status ON deployment (status, created_at DESC);
