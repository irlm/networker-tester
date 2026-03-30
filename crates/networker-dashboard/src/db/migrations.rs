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

/// V005 migration: Add packet_capture_json column to TestRun for tshark capture summaries.
const V005_PACKET_CAPTURE: &str = r#"
ALTER TABLE TestRun ADD COLUMN IF NOT EXISTS packet_capture_json JSONB;
"#;

/// V006 migration: Extend schedule table for scheduler feature.
const V006_SCHEDULES: &str = r#"
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS auto_start_vm BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS auto_stop_vm BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS deployment_id UUID REFERENCES deployment(deployment_id);
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS name VARCHAR(200);
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS config JSONB;
ALTER TABLE schedule ALTER COLUMN definition_id DROP NOT NULL;
"#;

/// V007 migration: Password reset tokens and email requirement.
const V007_PASSWORD_RESET: &str = r#"
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS password_reset_token VARCHAR(128);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS password_reset_expires TIMESTAMPTZ;
"#;

/// V008 migration: Email-based identity — drop username, enforce email NOT NULL + UNIQUE,
/// add status/auth_provider/sso columns, migrate disabled → status.
const V008_EMAIL_IDENTITY: &str = r#"
BEGIN;
-- Step 1: Backfill email from username (BEFORE dropping username)
UPDATE dash_user SET email = username WHERE email IS NULL OR email = '';
-- Step 2: Enforce NOT NULL + UNIQUE on email
ALTER TABLE dash_user ALTER COLUMN email SET NOT NULL;
ALTER TABLE dash_user ADD CONSTRAINT dash_user_email_unique UNIQUE (email);
-- Step 3: Drop username
ALTER TABLE dash_user DROP COLUMN IF EXISTS username;
-- Step 4: Allow NULL password for SSO accounts
ALTER TABLE dash_user ALTER COLUMN password_hash DROP NOT NULL;
-- Step 5: Add new columns
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'pending';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS auth_provider VARCHAR(20) NOT NULL DEFAULT 'local';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_subject_id VARCHAR(255);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS display_name VARCHAR(200);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS avatar_url TEXT;
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_only BOOLEAN NOT NULL DEFAULT FALSE;
-- Step 6: Migrate disabled -> status
UPDATE dash_user SET status = 'active' WHERE disabled = FALSE;
UPDATE dash_user SET status = 'disabled' WHERE disabled = TRUE;
ALTER TABLE dash_user DROP COLUMN IF EXISTS disabled;
-- Step 7: Invalidate existing plaintext reset tokens
UPDATE dash_user SET password_reset_token = NULL, password_reset_expires = NULL;
-- Step 8: Indexes
CREATE INDEX IF NOT EXISTS ix_user_sso ON dash_user (auth_provider, sso_subject_id) WHERE sso_subject_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_user_status ON dash_user (status);
COMMIT;
"#;

/// V009 migration: Cloud connections for identity federation (no stored credentials).
const V009_CLOUD_CONNECTIONS: &str = r#"
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
"#;

/// V010 migration: Multi-project tenancy, cloud accounts, share links, command approval.
const V010_MULTI_PROJECT: &str = r#"
-- V010: Multi-project tenancy, cloud accounts, share links

-- 1. Projects table
CREATE TABLE IF NOT EXISTS project (
    project_id   UUID           NOT NULL PRIMARY KEY,
    name         VARCHAR(200)   NOT NULL,
    slug         VARCHAR(100)   NOT NULL UNIQUE,
    description  TEXT,
    created_by   UUID           REFERENCES dash_user(user_id),
    created_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    settings     JSONB          NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX IF NOT EXISTS ix_project_slug ON project (slug);

-- 2. Project membership
CREATE TABLE IF NOT EXISTS project_member (
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    user_id      UUID           NOT NULL REFERENCES dash_user(user_id) ON DELETE CASCADE,
    role         VARCHAR(20)    NOT NULL DEFAULT 'viewer',
    joined_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    invited_by   UUID           REFERENCES dash_user(user_id),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX IF NOT EXISTS ix_project_member_user ON project_member (user_id);

-- 3. Cloud accounts (project-scoped, encrypted credentials)
CREATE TABLE IF NOT EXISTS cloud_account (
    account_id       UUID           NOT NULL PRIMARY KEY,
    project_id       UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    owner_id         UUID           REFERENCES dash_user(user_id) ON DELETE CASCADE,
    name             VARCHAR(200)   NOT NULL,
    provider         VARCHAR(20)    NOT NULL,
    credentials_enc  BYTEA          NOT NULL,
    credentials_nonce BYTEA         NOT NULL,
    region_default   VARCHAR(100),
    status           VARCHAR(20)    NOT NULL DEFAULT 'active',
    last_validated   TIMESTAMPTZ,
    validation_error TEXT,
    created_at       TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_cloud_account_project ON cloud_account (project_id);
CREATE INDEX IF NOT EXISTS ix_cloud_account_owner ON cloud_account (owner_id) WHERE owner_id IS NOT NULL;

-- 4. Share links
CREATE TABLE IF NOT EXISTS share_link (
    link_id      UUID           NOT NULL PRIMARY KEY,
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    token_hash   VARCHAR(64)    NOT NULL UNIQUE,
    resource_type VARCHAR(20)   NOT NULL,
    resource_id  UUID,
    label        VARCHAR(200),
    expires_at   TIMESTAMPTZ    NOT NULL,
    created_by   UUID           NOT NULL REFERENCES dash_user(user_id),
    created_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    revoked      BOOLEAN        NOT NULL DEFAULT FALSE,
    access_count INT            NOT NULL DEFAULT 0,
    last_accessed TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS ix_share_link_token ON share_link (token_hash) WHERE revoked = FALSE;
CREATE INDEX IF NOT EXISTS ix_share_link_project ON share_link (project_id, resource_type);
CREATE INDEX IF NOT EXISTS ix_share_link_expires ON share_link (expires_at) WHERE revoked = FALSE;

-- 5. Command approval
CREATE TABLE IF NOT EXISTS command_approval (
    approval_id  UUID           NOT NULL PRIMARY KEY,
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    agent_id     UUID           NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
    command_type VARCHAR(50)    NOT NULL,
    command_detail JSONB        NOT NULL,
    status       VARCHAR(20)    NOT NULL DEFAULT 'pending',
    requested_by UUID           NOT NULL REFERENCES dash_user(user_id),
    decided_by   UUID           REFERENCES dash_user(user_id),
    requested_at TIMESTAMPTZ    NOT NULL DEFAULT now(),
    decided_at   TIMESTAMPTZ,
    expires_at   TIMESTAMPTZ    NOT NULL,
    reason       TEXT
);
CREATE INDEX IF NOT EXISTS ix_command_approval_pending ON command_approval (project_id, status) WHERE status = 'pending';

-- 6. Test visibility rules
CREATE TABLE IF NOT EXISTS test_visibility_rule (
    rule_id       UUID           NOT NULL PRIMARY KEY,
    project_id    UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    user_id       UUID           REFERENCES dash_user(user_id) ON DELETE CASCADE,
    resource_type VARCHAR(20)    NOT NULL,
    resource_id   UUID           NOT NULL,
    created_by    UUID           NOT NULL REFERENCES dash_user(user_id),
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_visibility_project ON test_visibility_rule (project_id, user_id, resource_type);

-- 7. Add nullable project_id FK to all resource tables
ALTER TABLE agent ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE test_definition ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE job ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE deployment ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE cloud_connection ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);

-- 8. Add cloud_account_id to deployment
ALTER TABLE deployment ADD COLUMN IF NOT EXISTS cloud_account_id UUID REFERENCES cloud_account(account_id);

-- 9. Add is_platform_admin to dash_user
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS is_platform_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- 10. Migrate existing admin users to platform admin
UPDATE dash_user SET is_platform_admin = TRUE WHERE role = 'admin';

-- 11. Create "Default" project (well-known UUID, idempotent)
INSERT INTO project (project_id, name, slug, description)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Default',
    'default',
    'Auto-created during v0.15 migration. Contains all pre-existing resources.'
) ON CONFLICT DO NOTHING;

-- 12. Move all existing resources into Default project
UPDATE agent SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE test_definition SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE job SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE schedule SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE deployment SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE cloud_connection SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;

-- 13. Add all existing active users to Default project preserving current role
INSERT INTO project_member (project_id, user_id, role)
SELECT
    '00000000-0000-0000-0000-000000000001',
    user_id,
    CASE role
        WHEN 'admin' THEN 'admin'
        WHEN 'operator' THEN 'operator'
        ELSE 'viewer'
    END
FROM dash_user
WHERE status = 'active'
ON CONFLICT DO NOTHING;

-- 14. Project-scoped indexes on resource tables
CREATE INDEX IF NOT EXISTS ix_agent_project ON agent (project_id);
CREATE INDEX IF NOT EXISTS ix_test_def_project ON test_definition (project_id);
CREATE INDEX IF NOT EXISTS ix_job_project ON job (project_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_schedule_project ON schedule (project_id) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS ix_deployment_project ON deployment (project_id, status, created_at DESC);
"#;

/// V012 migration: Workspace management — invites, warnings, soft-delete.
const V012_WORKSPACE_MANAGEMENT: &str = r#"
-- V012: Workspace management — invites, warnings, soft-delete

CREATE TABLE IF NOT EXISTS workspace_invite (
    invite_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    email       VARCHAR(255) NOT NULL,
    role        VARCHAR(20) NOT NULL DEFAULT 'viewer',
    token_hash  VARCHAR(128) NOT NULL,
    status      VARCHAR(20) NOT NULL DEFAULT 'pending',
    invited_by  UUID NOT NULL REFERENCES dash_user(user_id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    accepted_by UUID REFERENCES dash_user(user_id)
);
CREATE INDEX IF NOT EXISTS ix_workspace_invite_token ON workspace_invite (token_hash);
CREATE INDEX IF NOT EXISTS ix_workspace_invite_project ON workspace_invite (project_id, status);

CREATE TABLE IF NOT EXISTS workspace_warning (
    warning_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id   UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    warning_type VARCHAR(30) NOT NULL,
    sent_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_workspace_warning_unique ON workspace_warning (project_id, warning_type);

ALTER TABLE project ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;
ALTER TABLE project ADD COLUMN IF NOT EXISTS delete_protection BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE project SET delete_protection = TRUE WHERE project_id = '00000000-0000-0000-0000-000000000001';
"#;

/// V014 migration: Project-scoped shared benchmark compare presets.
const V014_BENCHMARK_COMPARE_PRESETS: &str = r#"
CREATE TABLE IF NOT EXISTS benchmark_compare_preset (
    preset_id        UUID           NOT NULL PRIMARY KEY,
    project_id       UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    created_by       UUID           NOT NULL REFERENCES dash_user(user_id),
    name             VARCHAR(200)   NOT NULL,
    name_key         VARCHAR(200)   NOT NULL,
    run_ids          UUID[]         NOT NULL,
    baseline_run_id  UUID           NOT NULL,
    target_search    VARCHAR(200)   NOT NULL DEFAULT '',
    scenario         VARCHAR(100)   NOT NULL DEFAULT '',
    phase_model      VARCHAR(200)   NOT NULL DEFAULT '',
    server_region    VARCHAR(100)   NOT NULL DEFAULT '',
    network_type     VARCHAR(50)    NOT NULL DEFAULT '',
    created_at       TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_benchmark_compare_preset_name
    ON benchmark_compare_preset (project_id, name_key);
CREATE INDEX IF NOT EXISTS ix_benchmark_compare_preset_project_updated
    ON benchmark_compare_preset (project_id, updated_at DESC);
"#;

/// V011 migration: Enforce NOT NULL on project_id columns (after soak period).
const V011_NOT_NULL_PROJECT_ID: &str = r#"
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM agent WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found agent rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM job WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found job rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM schedule WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found schedule rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM deployment WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found deployment rows with NULL project_id — run backfill first';
    END IF;
END $$;

ALTER TABLE agent ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE job ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE schedule ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE deployment ALTER COLUMN project_id SET NOT NULL;
"#;

/// V013 migration: Benchmark tables for AletheBench results.
const V013_BENCHMARKS: &str = r#"
CREATE TABLE IF NOT EXISTS benchmark_run (
    run_id      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name        VARCHAR(200) NOT NULL,
    config      JSONB NOT NULL DEFAULT '{}',
    status      VARCHAR(20) NOT NULL DEFAULT 'running',
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    tier        VARCHAR(20) DEFAULT 'core',
    created_by  UUID REFERENCES dash_user(user_id)
);

CREATE TABLE IF NOT EXISTS benchmark_result (
    result_id    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id       UUID NOT NULL REFERENCES benchmark_run(run_id) ON DELETE CASCADE,
    language     VARCHAR(50) NOT NULL,
    runtime      VARCHAR(50) NOT NULL,
    server_os    VARCHAR(50) DEFAULT 'ubuntu-24.04',
    client_os    VARCHAR(50) DEFAULT 'ubuntu-24.04',
    cloud        VARCHAR(20) DEFAULT 'azure',
    phase        VARCHAR(10) DEFAULT 'warm',
    concurrency  INTEGER DEFAULT 1,
    metrics      JSONB NOT NULL DEFAULT '{}',
    started_at   TIMESTAMPTZ DEFAULT now(),
    finished_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS ix_benchmark_result_run ON benchmark_result(run_id);
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

    // V005: packet_capture_json column on TestRun
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 5", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V005 packet_capture migration...");
        client.batch_execute(V005_PACKET_CAPTURE).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (5) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V005 migration complete");
    }

    // V006: Extend schedule table for scheduler feature
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 6", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V006 schedules migration...");
        client.batch_execute(V006_SCHEDULES).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (6) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V006 migration complete");
    }

    // V007: Password reset tokens
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 7", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V007 password_reset migration...");
        client.batch_execute(V007_PASSWORD_RESET).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (7) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V007 migration complete");
    }

    // V008: Email-based identity
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 8", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V008 email_identity migration...");
        client.batch_execute(V008_EMAIL_IDENTITY).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (8) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V008 migration complete");
    }

    // V009: Cloud connections
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 9", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V009 cloud_connections migration...");
        client.batch_execute(V009_CLOUD_CONNECTIONS).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (9) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V009 migration complete");
    }

    // V010: Multi-project tenancy
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 10", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V010 multi-project migration...");
        client.batch_execute(V010_MULTI_PROJECT).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (10) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V010 migration complete");
    }

    // V011: Enforce NOT NULL on project_id columns
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 11", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V011 NOT NULL project_id migration...");
        client.batch_execute(V011_NOT_NULL_PROJECT_ID).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (11) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V011 migration complete");
    }

    // V012: Workspace management — invites, warnings, soft-delete
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 12", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V012 workspace_management migration...");
        client.batch_execute(V012_WORKSPACE_MANAGEMENT).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (12) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V012 migration complete");
    }

    // V013: Benchmark tables for AletheBench
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 13", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V013 benchmarks migration...");
        client.batch_execute(V013_BENCHMARKS).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (13) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V013 migration complete");
    }

    // V014: Shared benchmark compare presets
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 14", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V014 benchmark_compare_presets migration...");
        client.batch_execute(V014_BENCHMARK_COMPARE_PRESETS).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (14) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V014 migration complete");
    }

    // V015: TLS endpoint profile tables + job.tls_profile_run_id column
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 15", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V015 TLS profile tables migration...");
        client
            .batch_execute(
                "ALTER TABLE job ADD COLUMN IF NOT EXISTS tls_profile_run_id UUID;

                 CREATE TABLE IF NOT EXISTS TlsProfileRun (
                     Id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                     ProjectId UUID NOT NULL REFERENCES project(project_id),
                     JobId UUID REFERENCES job(job_id),
                     Host VARCHAR(255) NOT NULL,
                     Port INT NOT NULL DEFAULT 443,
                     TargetKind VARCHAR(50) NOT NULL DEFAULT 'external',
                     CoverageLevel VARCHAR(50) NOT NULL DEFAULT 'standard',
                     SummaryStatus VARCHAR(50) NOT NULL DEFAULT 'pending',
                     SummaryScore INT,
                     ProfileJson JSONB,
                     StartedAt TIMESTAMPTZ NOT NULL DEFAULT now(),
                     FinishedAt TIMESTAMPTZ,
                     CreatedBy UUID REFERENCES dash_user(user_id)
                 );

                 CREATE INDEX IF NOT EXISTS ix_tlsprofilerun_project
                     ON TlsProfileRun (ProjectId, StartedAt DESC);",
            )
            .await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (15) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V015 migration complete");
    }

    Ok(())
}
