use tokio_postgres::Client;

/// V002 migration: Dashboard platform tables.
/// V001 is the existing networker-tester schema (TestRun, RequestAttempt, etc.).
const V002_DASHBOARD: &str = r#"
-- Migration tracking
CREATE TABLE IF NOT EXISTS _migrations (
    version     INT          NOT NULL PRIMARY KEY,
    applied_at  TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- Users and authentication
CREATE TABLE IF NOT EXISTS dash_user (
    user_id       UUID           NOT NULL PRIMARY KEY,
    username      VARCHAR(100)   NOT NULL UNIQUE,
    email         VARCHAR(255),
    password_hash VARCHAR(255)   NOT NULL,
    role          VARCHAR(20)    NOT NULL DEFAULT 'viewer',
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ,
    disabled      BOOLEAN        NOT NULL DEFAULT FALSE
);

-- Test agents
CREATE TABLE IF NOT EXISTS agent (
    agent_id       UUID           NOT NULL PRIMARY KEY,
    name           VARCHAR(100)   NOT NULL,
    region         VARCHAR(100),
    provider       VARCHAR(20),
    status         VARCHAR(20)    NOT NULL DEFAULT 'offline',
    version        VARCHAR(50),
    os             VARCHAR(50),
    arch           VARCHAR(20),
    last_heartbeat TIMESTAMPTZ,
    registered_at  TIMESTAMPTZ    NOT NULL DEFAULT now(),
    api_key        VARCHAR(255)   NOT NULL UNIQUE,
    tags           JSONB
);

-- Test definitions (reusable, versioned)
CREATE TABLE IF NOT EXISTS test_definition (
    definition_id UUID           NOT NULL PRIMARY KEY,
    name          VARCHAR(200)   NOT NULL,
    description   TEXT,
    config        JSONB          NOT NULL,
    version       INT            NOT NULL DEFAULT 1,
    created_by    UUID           REFERENCES dash_user(user_id),
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ    NOT NULL DEFAULT now()
);

-- Jobs (test execution requests)
CREATE TABLE IF NOT EXISTS job (
    job_id        UUID           NOT NULL PRIMARY KEY,
    definition_id UUID           REFERENCES test_definition(definition_id),
    agent_id      UUID           REFERENCES agent(agent_id),
    status        VARCHAR(20)    NOT NULL DEFAULT 'pending',
    config        JSONB          NOT NULL,
    created_by    UUID           REFERENCES dash_user(user_id),
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    started_at    TIMESTAMPTZ,
    finished_at   TIMESTAMPTZ,
    run_id        UUID,
    error_message TEXT
);

-- Schedules (recurring jobs)
CREATE TABLE IF NOT EXISTS schedule (
    schedule_id   UUID           NOT NULL PRIMARY KEY,
    definition_id UUID           NOT NULL REFERENCES test_definition(definition_id),
    agent_id      UUID           REFERENCES agent(agent_id),
    cron_expr     VARCHAR(100)   NOT NULL,
    enabled       BOOLEAN        NOT NULL DEFAULT TRUE,
    created_by    UUID           REFERENCES dash_user(user_id),
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    next_run_at   TIMESTAMPTZ,
    last_run_at   TIMESTAMPTZ
);

-- Indexes
CREATE INDEX IF NOT EXISTS ix_job_status ON job (status, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_job_agent ON job (agent_id, status);
CREATE INDEX IF NOT EXISTS ix_schedule_next ON schedule (next_run_at) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS ix_agent_status ON agent (status);
"#;

/// V003 migration: Deployment table for install.sh --deploy integration.
const V003_DEPLOYMENTS: &str = r#"
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
"#;

/// V004 migration: Add must_change_password flag for forced password change on first login.
const V004_MUST_CHANGE_PASSWORD: &str = r#"
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS must_change_password BOOLEAN NOT NULL DEFAULT FALSE;
"#;

/// Run pending migrations.
pub async fn run(client: &Client) -> anyhow::Result<()> {
    // Ensure migration tracking table exists
    client
        .execute(
            "CREATE TABLE IF NOT EXISTS _migrations (
                version INT NOT NULL PRIMARY KEY,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            &[],
        )
        .await?;

    // Check if V002 already applied
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 2", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V002 dashboard migration...");
        client.batch_execute(V002_DASHBOARD).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (2) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V002 migration complete");
    }

    // V003: Deployment table
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 3", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V003 deployments migration...");
        client.batch_execute(V003_DEPLOYMENTS).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (3) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V003 migration complete");
    }

    // V004: must_change_password flag
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 4", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V004 must_change_password migration...");
        client.batch_execute(V004_MUST_CHANGE_PASSWORD).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (4) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V004 migration complete");
    }

    Ok(())
}
