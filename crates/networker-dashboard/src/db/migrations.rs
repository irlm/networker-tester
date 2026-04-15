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
/// Guarded with DO $$ block — TestRun may not exist on fresh dashboard-only installs.
const V005_PACKET_CAPTURE: &str = r#"
DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'testrun') THEN
    ALTER TABLE TestRun ADD COLUMN IF NOT EXISTS packet_capture_json JSONB;
  END IF;
END $$;
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

/// V016 migration: Benchmark creation — VM catalog, config, and cell tables.
const V016_BENCHMARK_CREATION: &str = r#"
-- VM catalog: registry of known VMs with pre-deployed languages
CREATE TABLE IF NOT EXISTS benchmark_vm_catalog (
    vm_id              UUID           NOT NULL PRIMARY KEY,
    project_id         UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    name               VARCHAR(200)   NOT NULL,
    cloud              VARCHAR(20)    NOT NULL,
    region             VARCHAR(100)   NOT NULL,
    ip                 VARCHAR(200)   NOT NULL,
    ssh_user           VARCHAR(100)   NOT NULL DEFAULT 'azureuser',
    languages          JSONB          NOT NULL DEFAULT '[]'::jsonb,
    vm_size            VARCHAR(100),
    status             VARCHAR(20)    NOT NULL DEFAULT 'unknown',
    last_health_check  TIMESTAMPTZ,
    created_by         UUID           REFERENCES dash_user(user_id),
    created_at         TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_benchmark_vm_catalog_project ON benchmark_vm_catalog (project_id);

-- Benchmark config: the user's benchmark request from the wizard
CREATE TABLE IF NOT EXISTS benchmark_config (
    config_id          UUID           NOT NULL PRIMARY KEY,
    project_id         UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    name               VARCHAR(200)   NOT NULL,
    template           VARCHAR(50),
    status             VARCHAR(30)    NOT NULL DEFAULT 'draft',
    created_by         UUID           REFERENCES dash_user(user_id),
    created_at         TIMESTAMPTZ    NOT NULL DEFAULT now(),
    started_at         TIMESTAMPTZ,
    finished_at        TIMESTAMPTZ,
    config_json        JSONB          NOT NULL DEFAULT '{}'::jsonb,
    error_message      TEXT,
    max_duration_secs  INT            NOT NULL DEFAULT 14400,
    baseline_run_id    UUID,
    worker_id          VARCHAR(100),
    last_heartbeat     TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS ix_benchmark_config_project ON benchmark_config (project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_benchmark_config_status ON benchmark_config (status) WHERE status IN ('queued', 'running');

-- Benchmark cell: one cloud/region/topology unit within a benchmark
CREATE TABLE IF NOT EXISTS benchmark_cell (
    cell_id            UUID           NOT NULL PRIMARY KEY,
    config_id          UUID           NOT NULL REFERENCES benchmark_config(config_id) ON DELETE CASCADE,
    cloud              VARCHAR(20)    NOT NULL,
    region             VARCHAR(100)   NOT NULL,
    topology           VARCHAR(20)    NOT NULL DEFAULT 'loopback',
    endpoint_vm_id     VARCHAR(200),
    tester_vm_id       VARCHAR(200),
    endpoint_ip        VARCHAR(200),
    tester_ip          VARCHAR(200),
    status             VARCHAR(30)    NOT NULL DEFAULT 'pending',
    languages          JSONB          NOT NULL DEFAULT '[]'::jsonb,
    vm_size            VARCHAR(100)
);
CREATE INDEX IF NOT EXISTS ix_benchmark_cell_config ON benchmark_cell (config_id);
"#;

/// V017 migration: Add cell_id/config_id to benchmark_run for cross-cell grouping.
const V017_BENCHMARK_RUN_CELL_LINK: &str = r#"
ALTER TABLE benchmark_run ADD COLUMN IF NOT EXISTS cell_id UUID;
ALTER TABLE benchmark_run ADD COLUMN IF NOT EXISTS config_id UUID;
CREATE INDEX IF NOT EXISTS ix_benchmark_run_config ON benchmark_run (config_id) WHERE config_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_benchmark_run_cell ON benchmark_run (cell_id) WHERE cell_id IS NOT NULL;
"#;

/// V018 migration: Scheduled benchmarks + regression detection.
const V018_SCHEDULED_BENCHMARKS_REGRESSION: &str = r#"
-- Allow schedules to reference a benchmark config template
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS benchmark_config_id UUID REFERENCES benchmark_config(config_id);

-- Regression detection results
CREATE TABLE IF NOT EXISTS benchmark_regression (
    regression_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    config_id UUID NOT NULL REFERENCES benchmark_config(config_id) ON DELETE CASCADE,
    baseline_config_id UUID REFERENCES benchmark_config(config_id),
    language VARCHAR(100) NOT NULL,
    metric VARCHAR(50) NOT NULL,
    baseline_value DOUBLE PRECISION NOT NULL,
    current_value DOUBLE PRECISION NOT NULL,
    delta_percent DOUBLE PRECISION NOT NULL,
    severity VARCHAR(20) NOT NULL DEFAULT 'warning',
    detected_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_benchmark_regression_config ON benchmark_regression (config_id, detected_at DESC);
CREATE INDEX IF NOT EXISTS ix_benchmark_regression_project ON benchmark_regression (config_id);
"#;

/// V021 migration: Per-request benchmark progress tracking.
const V021_BENCHMARK_REQUEST_PROGRESS: &str = r#"
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
"#;

/// V022 migration: Application benchmark mode — add benchmark_type, proxies, tester_os columns.
const V022_APPLICATION_BENCHMARK: &str = "
    ALTER TABLE benchmark_config
        ADD COLUMN IF NOT EXISTS benchmark_type TEXT NOT NULL DEFAULT 'fullstack';

    ALTER TABLE benchmark_testbed
        ADD COLUMN IF NOT EXISTS proxies JSONB NOT NULL DEFAULT '[]'::jsonb,
        ADD COLUMN IF NOT EXISTS tester_os TEXT NOT NULL DEFAULT 'server';

    CREATE INDEX IF NOT EXISTS ix_benchmark_config_type
        ON benchmark_config (benchmark_type);
";

/// V023 migration: Performance log table for API + render timing telemetry.
const V023_PERF_LOG: &str = "
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
";

/// V024 migration: Sovereignty zones + server registry tables.
const V024_SOVEREIGNTY_ZONES: &str = r#"
CREATE TABLE IF NOT EXISTS sovereignty_zone (
    code              CHAR(2)       NOT NULL PRIMARY KEY,
    parent_code       CHAR(2),
    name              VARCHAR(50)   NOT NULL UNIQUE,
    display           VARCHAR(100)  NOT NULL,
    legal_note        VARCHAR(255),
    compliance_level  VARCHAR(100),
    fallback_zone     CHAR(2),
    auto_detect       JSONB         NOT NULL DEFAULT '{}',
    requires_approval BOOLEAN       NOT NULL DEFAULT FALSE,
    requires_mfa      BOOLEAN       NOT NULL DEFAULT FALSE,
    status            VARCHAR(20)   NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ   NOT NULL DEFAULT now(),
    FOREIGN KEY (parent_code) REFERENCES sovereignty_zone(code),
    FOREIGN KEY (fallback_zone) REFERENCES sovereignty_zone(code)
);

CREATE TABLE IF NOT EXISTS server_registry (
    server_id   CHAR(3)       NOT NULL,
    zone_code   CHAR(2)       NOT NULL,
    hostname    VARCHAR(255)  NOT NULL,
    endpoint    VARCHAR(255)  NOT NULL,
    internal_ip VARCHAR(45),
    db_url      VARCHAR(500),
    status      VARCHAR(20)   NOT NULL DEFAULT 'active',
    last_health TIMESTAMPTZ,
    priority    INT           NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ   NOT NULL DEFAULT now(),
    PRIMARY KEY (zone_code, server_id),
    FOREIGN KEY (zone_code) REFERENCES sovereignty_zone(code)
);
CREATE INDEX IF NOT EXISTS ix_server_registry_status ON server_registry(zone_code, status);

CREATE TABLE IF NOT EXISTS project_routing (
    project_id   CHAR(14)    NOT NULL PRIMARY KEY,
    home_zone    CHAR(2)     NOT NULL,
    current_zone CHAR(2)     NOT NULL,
    migrated_at  TIMESTAMPTZ,
    migrated_by  UUID,
    FOREIGN KEY (home_zone) REFERENCES sovereignty_zone(code),
    FOREIGN KEY (current_zone) REFERENCES sovereignty_zone(code)
);
CREATE INDEX IF NOT EXISTS ix_project_routing_current ON project_routing(current_zone, home_zone);

CREATE TABLE IF NOT EXISTS migration_request (
    request_id   UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id   CHAR(14)    NOT NULL,
    from_zone    CHAR(2)     NOT NULL,
    to_zone      CHAR(2)     NOT NULL,
    reason       TEXT        NOT NULL,
    requested_by UUID        NOT NULL,
    approved_by  UUID,
    status       VARCHAR(20) NOT NULL DEFAULT 'pending',
    scheduled_at TIMESTAMPTZ,
    started_at   TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    data_size_mb BIGINT,
    error_message TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (requested_by) REFERENCES dash_user(user_id),
    FOREIGN KEY (approved_by) REFERENCES dash_user(user_id),
    FOREIGN KEY (from_zone) REFERENCES sovereignty_zone(code),
    FOREIGN KEY (to_zone) REFERENCES sovereignty_zone(code)
);

CREATE TABLE IF NOT EXISTS migration_audit_log (
    log_id      UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id  UUID        NOT NULL,
    step        VARCHAR(50) NOT NULL,
    status      VARCHAR(20) NOT NULL,
    details     JSONB,
    checksum    VARCHAR(128),
    duration_ms BIGINT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id) REFERENCES migration_request(request_id)
);
CREATE INDEX IF NOT EXISTS ix_migration_audit_request ON migration_audit_log(request_id, created_at);
"#;

/// V024 seed: 32 sovereignty zones + current US East server.
/// Rows with no fallback_zone dependency are inserted first (us, sg),
/// then all others that reference them, using ON CONFLICT DO NOTHING for idempotency.
const V024_SEED_ZONES: &str = r#"
-- Insert root zones first (no fallback dependencies)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('us', 'USA Commercial',           'USA Commercial',                     NULL, FALSE, FALSE),
  ('sg', 'Singapore + ASEAN',        'Singapore + ASEAN',                  'us', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend only on us or sg
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('ug', 'USA GovCloud',             'USA GovCloud',                       'us', TRUE,  FALSE),
  ('uh', 'USA Healthcare',           'USA Healthcare',                     'us', TRUE,  FALSE),
  ('ca', 'Canada',                   'Canada',                             'us', FALSE, FALSE),
  ('mx', 'Mexico + Central Am + Caribbean', 'Mexico, Central America & Caribbean', 'us', FALSE, FALSE),
  ('sa', 'South America excl Brasil','South America (excl Brasil)',         'us', FALSE, FALSE),
  ('br', 'Brasil',                   'Brasil',                             'us', FALSE, FALSE),
  ('eu', 'Europe EU + EEA',          'Europe EU + EEA',                    'us', FALSE, FALSE),
  ('uk', 'United Kingdom',           'United Kingdom',                     'eu', FALSE, FALSE),
  ('jp', 'Japan',                    'Japan',                              'sg', FALSE, FALSE),
  ('in', 'India',                    'India',                              'sg', FALSE, FALSE),
  ('id', 'Indonesia',                'Indonesia',                          'sg', FALSE, FALSE),
  ('vn', 'Vietnam',                  'Vietnam',                            'sg', FALSE, FALSE),
  ('tw', 'Taiwan',                   'Taiwan',                             'sg', FALSE, FALSE),
  ('hk', 'Hong Kong',                'Hong Kong',                          'sg', FALSE, FALSE),
  ('ph', 'Philippines',              'Philippines',                        'sg', FALSE, FALSE),
  ('au', 'Australia + NZ',           'Australia + New Zealand',            'sg', FALSE, FALSE),
  ('af', 'Africa general',           'Africa',                             'eu', FALSE, FALSE),
  ('me', 'Middle East UAE/Saudi',    'Middle East (UAE/Saudi)',             'eu', FALSE, FALSE),
  ('il', 'Israel',                   'Israel',                             'eu', FALSE, FALSE),
  ('gl', 'Global / no residency',    'Global (No Data Residency)',         'us', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on ug (which depends on us)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('ud', 'USA DoD',                  'USA DoD',                            'ug', TRUE,  TRUE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on eu
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('es', 'Europe Sovereign',         'Europe Sovereign',                   'eu', TRUE,  FALSE),
  ('ru', 'Russia + CIS',             'Russia + CIS',                       'eu', TRUE,  FALSE),
  ('cn', 'China Commercial',         'China Commercial',                   'sg', TRUE,  FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on cn (which depends on sg)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('cg', 'China Government',         'China Government',                   'cn', TRUE,  TRUE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on jp (which depends on sg)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('kr', 'South Korea',              'South Korea',                        'jp', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on af (which depends on eu)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('ng', 'Nigeria',                  'Nigeria',                            'af', FALSE, FALSE),
  ('za', 'South Africa',             'South Africa',                       'af', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on me (which depends on eu)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('qa', 'Qatar + Gulf',             'Qatar + Gulf',                       'me', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Seed current US East server
INSERT INTO server_registry (server_id, zone_code, hostname, endpoint, internal_ip, db_url, status)
VALUES ('a20', 'us', 'alethedash-vm', 'https://alethedash.com', '20.42.8.158',
        'postgres://alethedash:alethedash@127.0.0.1/alethedash', 'active')
ON CONFLICT DO NOTHING;
"#;

/// V026: System health tracking table.
const V026_SYSTEM_HEALTH: &str = r#"
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
"#;

/// V028: Partial index for dispatcher sweep query — dramatically speeds up
/// `SELECT ... WHERE status = 'queued' ORDER BY queued_at` when most rows
/// have long since reached a terminal state.
const V028_DISPATCHER_INDEX: &str = r#"
CREATE INDEX IF NOT EXISTS idx_benchmark_config_queued
    ON benchmark_config (tester_id, queued_at NULLS LAST)
    WHERE status = 'queued';
"#;

/// V029: Link project_tester to cloud_connection for secretless provisioning.
const V029_TESTER_CLOUD_CONN: &str = r#"
-- V029: Link testers to cloud_connections for secretless provisioning.
ALTER TABLE project_tester
  ADD COLUMN IF NOT EXISTS cloud_connection_id UUID
    REFERENCES cloud_connection(connection_id) ON DELETE RESTRICT;

CREATE INDEX IF NOT EXISTS idx_project_tester_cloud_conn
  ON project_tester(cloud_connection_id)
  WHERE cloud_connection_id IS NOT NULL;
"#;

/// V030: Dynamic SSO providers, system config, and project member status.
const V030_SSO_AND_MEMBER_STATUS: &str = r#"
-- V030: Dynamic SSO providers, system config, and project member status.

-- 1. SSO providers (replaces env-var SSO config)
CREATE TABLE IF NOT EXISTS sso_provider (
    provider_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                 VARCHAR(200) NOT NULL,
    provider_type        VARCHAR(30)  NOT NULL,
    client_id            TEXT         NOT NULL,
    client_secret_enc    BYTEA        NOT NULL,
    client_secret_nonce  BYTEA        NOT NULL,
    issuer_url           TEXT,
    tenant_id            TEXT,
    extra_config         JSONB        NOT NULL DEFAULT '{}',
    enabled              BOOLEAN      NOT NULL DEFAULT TRUE,
    display_order        SMALLINT     NOT NULL DEFAULT 0,
    created_by           UUID         REFERENCES dash_user(user_id),
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- 2. Platform-level system config (public_url, etc.)
CREATE TABLE IF NOT EXISTS system_config (
    key         VARCHAR(100) PRIMARY KEY,
    value       TEXT         NOT NULL,
    updated_by  UUID         REFERENCES dash_user(user_id),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- 3. Project member status + invite tracking
ALTER TABLE project_member
  ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'active',
  ADD COLUMN IF NOT EXISTS invite_sent_at TIMESTAMPTZ;
"#;

/// V027: Persistent testers — project_tester table, benchmark_config tester link,
/// phase columns on progress-tracking tables.
const V027_PERSISTENT_TESTERS: &str = r#"
-- V027: Persistent testers — project_tester table, benchmark_config tester link,
-- phase columns on progress-tracking tables. Idempotent.

CREATE TABLE IF NOT EXISTS project_tester (
    tester_id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id          TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,

    -- identity
    name                TEXT NOT NULL,
    cloud               TEXT NOT NULL,
    region              TEXT NOT NULL,
    vm_size             TEXT NOT NULL DEFAULT 'Standard_D2s_v3',

    -- cloud resource handles
    vm_name             TEXT,
    vm_resource_id      TEXT,
    public_ip           INET,
    ssh_user            TEXT NOT NULL DEFAULT 'azureuser',

    -- two orthogonal state axes
    power_state         TEXT NOT NULL DEFAULT 'provisioning',
    allocation          TEXT NOT NULL DEFAULT 'idle',
    status_message      TEXT,
    locked_by_config_id UUID,

    -- version tracking
    installer_version   TEXT,
    last_installed_at   TIMESTAMPTZ,

    -- auto-shutdown schedule
    auto_shutdown_enabled    BOOLEAN  NOT NULL DEFAULT TRUE,
    auto_shutdown_local_hour SMALLINT NOT NULL DEFAULT 23,
    next_shutdown_at         TIMESTAMPTZ,
    shutdown_deferral_count  SMALLINT NOT NULL DEFAULT 0,

    -- recovery
    auto_probe_enabled  BOOLEAN NOT NULL DEFAULT FALSE,

    -- usage
    last_used_at                   TIMESTAMPTZ,
    avg_benchmark_duration_seconds INTEGER,
    benchmark_run_count            INTEGER NOT NULL DEFAULT 0,

    -- audit
    created_by          UUID NOT NULL REFERENCES dash_user(user_id),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE (project_id, name),

    CONSTRAINT lock_holder_implies_locked CHECK (
        (allocation = 'locked' AND locked_by_config_id IS NOT NULL)
     OR (allocation != 'locked' AND locked_by_config_id IS NULL)
    ),
    CONSTRAINT lock_requires_running_vm CHECK (
        allocation != 'locked' OR power_state = 'running'
    )
);

CREATE INDEX IF NOT EXISTS idx_project_tester_project    ON project_tester(project_id);
CREATE INDEX IF NOT EXISTS idx_project_tester_power      ON project_tester(power_state)  WHERE power_state IN ('running','stopped');
CREATE INDEX IF NOT EXISTS idx_project_tester_alloc      ON project_tester(allocation)   WHERE allocation IN ('idle','locked');
CREATE INDEX IF NOT EXISTS idx_project_tester_shutdown   ON project_tester(next_shutdown_at) WHERE auto_shutdown_enabled = TRUE;
CREATE INDEX IF NOT EXISTS idx_project_tester_last_used  ON project_tester(project_id, last_used_at DESC NULLS LAST);

-- benchmark_config new columns
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_id              UUID;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_name_snapshot   TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_region_snapshot TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_cloud_snapshot  TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_vm_size_snapshot TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS tester_version_snapshot TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS queued_at              TIMESTAMPTZ;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS current_phase          TEXT;
ALTER TABLE benchmark_config ADD COLUMN IF NOT EXISTS outcome                TEXT;

-- Idempotent FK: benchmark_config.tester_id -> project_tester(tester_id) ON DELETE SET NULL
DO $$ BEGIN
    ALTER TABLE benchmark_config
        ADD CONSTRAINT benchmark_config_tester_id_fkey
        FOREIGN KEY (tester_id) REFERENCES project_tester(tester_id) ON DELETE SET NULL;
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- Idempotent reverse FK: project_tester.locked_by_config_id -> benchmark_config(config_id) ON DELETE RESTRICT
DO $$ BEGIN
    ALTER TABLE project_tester
        ADD CONSTRAINT project_tester_locked_by_config_id_fkey
        FOREIGN KEY (locked_by_config_id) REFERENCES benchmark_config(config_id) ON DELETE RESTRICT;
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- Backfill legacy application benchmarks so the CHECK constraint passes.
UPDATE benchmark_config SET tester_name_snapshot = 'legacy-ephemeral-vm'
WHERE benchmark_type = 'application' AND tester_name_snapshot IS NULL;

-- Idempotent CHECK: application benchmarks need a tester link or snapshot
DO $$ BEGIN
    ALTER TABLE benchmark_config
        ADD CONSTRAINT app_configs_need_tester
        CHECK (
            benchmark_type != 'application'
            OR (tester_id IS NOT NULL OR tester_name_snapshot IS NOT NULL)
        );
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

-- Phase/outcome columns on other progress-tracking tables
ALTER TABLE job      ADD COLUMN IF NOT EXISTS current_phase TEXT;
ALTER TABLE job      ADD COLUMN IF NOT EXISTS outcome       TEXT;
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS current_phase TEXT;
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS outcome       TEXT;
"#;

/// V024b migration: Convert project_id from UUID to 14-char base36.
///
/// This is a Rust-driven migration because the base36 encoding + Damm check
/// digits can't be computed in pure SQL. It rewrites the PK on `project` and
/// every FK column that references it.
async fn migrate_project_ids(client: &Client) -> anyhow::Result<()> {
    use crate::project_id::ProjectId;

    // ── Step 1: Add temporary columns to the project table ──────────────
    client
        .batch_execute(
            "ALTER TABLE project ADD COLUMN IF NOT EXISTS new_project_id CHAR(14);
             ALTER TABLE project ADD COLUMN IF NOT EXISTS old_project_id UUID;",
        )
        .await?;

    // ── Step 2: Generate new IDs for each existing project ──────────────
    let projects = client
        .query("SELECT project_id, created_at FROM project", &[])
        .await?;

    for row in &projects {
        let old_id: uuid::Uuid = row.get("project_id");
        let created_at: chrono::DateTime<chrono::Utc> = row.get("created_at");
        let unix_secs = created_at.timestamp() as u64;

        let new_id = ProjectId::generate_deterministic("us", "a20", unix_secs);
        let new_id_str = new_id.as_str().to_string();

        client
            .execute(
                "UPDATE project SET new_project_id = $1, old_project_id = $2 WHERE project_id = $3",
                &[&new_id_str, &old_id, &old_id],
            )
            .await?;
    }

    tracing::info!(
        count = projects.len(),
        "Generated new base36 project IDs for all projects"
    );

    // ── Step 3: Add new_project_id to ALL FK tables ─────────────────────
    // Tables with NOT NULL project_id
    let not_null_fk_tables = [
        "project_member",
        "cloud_account",
        "share_link",
        "command_approval",
        "test_visibility_rule",
        "workspace_invite",
        "workspace_warning",
        "benchmark_compare_preset",
        "benchmark_vm_catalog",
        "benchmark_config",
    ];
    // Tables where project_id was nullable, then made NOT NULL by V011
    let v011_not_null_tables = ["agent", "job", "schedule", "deployment"];
    // Tables where project_id may still be nullable
    let nullable_fk_tables = ["test_definition", "cloud_connection"];

    // Add new_project_id column to all FK tables
    for table in not_null_fk_tables
        .iter()
        .chain(v011_not_null_tables.iter())
        .chain(nullable_fk_tables.iter())
    {
        let sql = format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS new_project_id CHAR(14);");
        client.batch_execute(&sql).await?;
    }

    // Also handle TlsProfileRun if it exists (mixed-case column name "ProjectId")
    client
        .batch_execute(
            "DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    ALTER TABLE tlsprofilerun ADD COLUMN IF NOT EXISTS new_project_id CHAR(14);
                END IF;
            END $$;",
        )
        .await?;

    // ── Step 4: Populate new_project_id by joining on project ───────────
    for table in not_null_fk_tables
        .iter()
        .chain(v011_not_null_tables.iter())
        .chain(nullable_fk_tables.iter())
    {
        let sql = format!(
            "UPDATE {table} t SET new_project_id = p.new_project_id \
             FROM project p WHERE t.project_id = p.project_id AND t.new_project_id IS NULL"
        );
        client.batch_execute(&sql).await?;
    }

    // TlsProfileRun uses mixed-case "ProjectId" — only if table exists
    client
        .batch_execute(
            "DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    UPDATE tlsprofilerun t SET new_project_id = p.new_project_id
                    FROM project p WHERE t.\"ProjectId\" = p.project_id AND t.new_project_id IS NULL;
                END IF;
            END $$;",
        )
        .await?;

    // ── Step 5: Drop old FK constraints ─────────────────────────────────
    // Auto-generated FK names from ALTER TABLE ... ADD COLUMN ... REFERENCES
    // are typically: {table}_{column}_fkey
    // Tables from V010 section 7 (ALTER TABLE ADD COLUMN project_id UUID REFERENCES ...):
    for table in &[
        "agent",
        "test_definition",
        "job",
        "schedule",
        "deployment",
        "cloud_connection",
    ] {
        let sql = format!("ALTER TABLE {table} DROP CONSTRAINT IF EXISTS {table}_project_id_fkey;");
        client.batch_execute(&sql).await?;
    }
    // Tables created in V010 with inline REFERENCES (CREATE TABLE ... project_id UUID NOT NULL REFERENCES ...):
    for table in &[
        "project_member",
        "cloud_account",
        "share_link",
        "command_approval",
        "test_visibility_rule",
    ] {
        let sql = format!("ALTER TABLE {table} DROP CONSTRAINT IF EXISTS {table}_project_id_fkey;");
        client.batch_execute(&sql).await?;
    }
    // V012 tables
    for table in &["workspace_invite", "workspace_warning"] {
        let sql = format!("ALTER TABLE {table} DROP CONSTRAINT IF EXISTS {table}_project_id_fkey;");
        client.batch_execute(&sql).await?;
    }
    // V014, V016 tables
    for table in &[
        "benchmark_compare_preset",
        "benchmark_vm_catalog",
        "benchmark_config",
    ] {
        let sql = format!("ALTER TABLE {table} DROP CONSTRAINT IF EXISTS {table}_project_id_fkey;");
        client.batch_execute(&sql).await?;
    }
    // TlsProfileRun (V015) — FK constraint name for mixed-case column — only if table exists
    client
        .batch_execute(
            "DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    ALTER TABLE tlsprofilerun DROP CONSTRAINT IF EXISTS \"TlsProfileRun_ProjectId_fkey\";
                    ALTER TABLE tlsprofilerun DROP CONSTRAINT IF EXISTS tlsprofilerun_projectid_fkey;
                END IF;
            END $$;",
        )
        .await?;

    // ── Step 6: Drop the project PK ─────────────────────────────────────
    client
        .batch_execute("ALTER TABLE project DROP CONSTRAINT IF EXISTS project_pkey;")
        .await?;

    // ── Step 7: Drop old columns, rename new columns ────────────────────
    // project table: drop old project_id, rename new_project_id → project_id
    client
        .batch_execute(
            "ALTER TABLE project DROP COLUMN project_id;
             ALTER TABLE project RENAME COLUMN new_project_id TO project_id;
             ALTER TABLE project ALTER COLUMN project_id SET NOT NULL;
             ALTER TABLE project ADD PRIMARY KEY (project_id);",
        )
        .await?;

    // All FK tables: drop old project_id, rename new_project_id → project_id, set NOT NULL where required
    for table in &not_null_fk_tables {
        let sql = format!(
            "ALTER TABLE {table} DROP COLUMN project_id; \
             ALTER TABLE {table} RENAME COLUMN new_project_id TO project_id; \
             ALTER TABLE {table} ALTER COLUMN project_id SET NOT NULL; \
             ALTER TABLE {table} ADD CONSTRAINT {table}_project_id_fkey \
                 FOREIGN KEY (project_id) REFERENCES project(project_id) ON DELETE CASCADE;"
        );
        client.batch_execute(&sql).await?;
    }

    for table in &v011_not_null_tables {
        let sql = format!(
            "ALTER TABLE {table} DROP COLUMN project_id; \
             ALTER TABLE {table} RENAME COLUMN new_project_id TO project_id; \
             ALTER TABLE {table} ALTER COLUMN project_id SET NOT NULL; \
             ALTER TABLE {table} ADD CONSTRAINT {table}_project_id_fkey \
                 FOREIGN KEY (project_id) REFERENCES project(project_id);"
        );
        client.batch_execute(&sql).await?;
    }

    for table in &nullable_fk_tables {
        let sql = format!(
            "ALTER TABLE {table} DROP COLUMN project_id; \
             ALTER TABLE {table} RENAME COLUMN new_project_id TO project_id; \
             ALTER TABLE {table} ADD CONSTRAINT {table}_project_id_fkey \
                 FOREIGN KEY (project_id) REFERENCES project(project_id);"
        );
        client.batch_execute(&sql).await?;
    }

    // TlsProfileRun: drop old ProjectId, rename new_project_id → ProjectId — only if table exists
    client
        .batch_execute(
            "DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    ALTER TABLE tlsprofilerun DROP COLUMN \"ProjectId\";
                    ALTER TABLE tlsprofilerun RENAME COLUMN new_project_id TO \"ProjectId\";
                    ALTER TABLE tlsprofilerun ALTER COLUMN \"ProjectId\" SET NOT NULL;
                    ALTER TABLE tlsprofilerun ADD CONSTRAINT tlsprofilerun_projectid_fkey
                        FOREIGN KEY (\"ProjectId\") REFERENCES project(project_id);
                END IF;
            END $$;",
        )
        .await?;

    // Fix project_member composite PK (was (project_id UUID, user_id UUID))
    client
        .batch_execute(
            "ALTER TABLE project_member DROP CONSTRAINT IF EXISTS project_member_pkey;
             ALTER TABLE project_member ADD PRIMARY KEY (project_id, user_id);",
        )
        .await?;

    // ── Step 8: Recreate indexes that referenced project_id ─────────────
    client
        .batch_execute(
            "DROP INDEX IF EXISTS ix_agent_project;
             CREATE INDEX IF NOT EXISTS ix_agent_project ON agent (project_id);
             DROP INDEX IF EXISTS ix_test_def_project;
             CREATE INDEX IF NOT EXISTS ix_test_def_project ON test_definition (project_id);
             DROP INDEX IF EXISTS ix_job_project;
             CREATE INDEX IF NOT EXISTS ix_job_project ON job (project_id, status, created_at DESC);
             DROP INDEX IF EXISTS ix_schedule_project;
             CREATE INDEX IF NOT EXISTS ix_schedule_project ON schedule (project_id) WHERE enabled = TRUE;
             DROP INDEX IF EXISTS ix_deployment_project;
             CREATE INDEX IF NOT EXISTS ix_deployment_project ON deployment (project_id, status, created_at DESC);
             DROP INDEX IF EXISTS ix_cloud_account_project;
             CREATE INDEX IF NOT EXISTS ix_cloud_account_project ON cloud_account (project_id);
             DROP INDEX IF EXISTS ix_share_link_project;
             CREATE INDEX IF NOT EXISTS ix_share_link_project ON share_link (project_id, resource_type);
             DROP INDEX IF EXISTS ix_command_approval_pending;
             CREATE INDEX IF NOT EXISTS ix_command_approval_pending ON command_approval (project_id, status) WHERE status = 'pending';
             DROP INDEX IF EXISTS ix_visibility_project;
             CREATE INDEX IF NOT EXISTS ix_visibility_project ON test_visibility_rule (project_id, user_id, resource_type);
             DROP INDEX IF EXISTS ix_project_member_user;
             CREATE INDEX IF NOT EXISTS ix_project_member_user ON project_member (user_id);
             DROP INDEX IF EXISTS ix_workspace_invite_project;
             CREATE INDEX IF NOT EXISTS ix_workspace_invite_project ON workspace_invite (project_id, status);
             DROP INDEX IF EXISTS ix_workspace_warning_unique;
             CREATE UNIQUE INDEX IF NOT EXISTS ix_workspace_warning_unique ON workspace_warning (project_id, warning_type);
             DROP INDEX IF EXISTS ix_benchmark_compare_preset_name;
             CREATE UNIQUE INDEX IF NOT EXISTS ix_benchmark_compare_preset_name ON benchmark_compare_preset (project_id, name_key);
             DROP INDEX IF EXISTS ix_benchmark_compare_preset_project_updated;
             CREATE INDEX IF NOT EXISTS ix_benchmark_compare_preset_project_updated ON benchmark_compare_preset (project_id, updated_at DESC);
             DROP INDEX IF EXISTS ix_benchmark_vm_catalog_project;
             CREATE INDEX IF NOT EXISTS ix_benchmark_vm_catalog_project ON benchmark_vm_catalog (project_id);
             DROP INDEX IF EXISTS ix_benchmark_config_project;
             CREATE INDEX IF NOT EXISTS ix_benchmark_config_project ON benchmark_config (project_id, created_at DESC);
",
        )
        .await?;

    // TlsProfileRun index — only if table exists
    client
        .batch_execute(
            "DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM pg_tables WHERE tablename = 'tlsprofilerun') THEN
                    DROP INDEX IF EXISTS ix_tlsprofilerun_project;
                    CREATE INDEX IF NOT EXISTS ix_tlsprofilerun_project ON tlsprofilerun (\"ProjectId\", \"StartedAt\" DESC);
                END IF;
            END $$;",
        )
        .await?;

    // ── Step 9: Populate project_routing (all projects → us/us) ─────────
    client
        .batch_execute(
            "INSERT INTO project_routing (project_id, home_zone, current_zone)
             SELECT project_id, 'us', 'us' FROM project
             ON CONFLICT DO NOTHING;",
        )
        .await?;

    // ── Step 10: Zone-prefix index for routing lookups ───────────────────
    client
        .batch_execute(
            "CREATE INDEX IF NOT EXISTS ix_project_zone_prefix ON project (substring(project_id from 1 for 2));",
        )
        .await?;

    // ── Step 11: Clean up old_project_id helper column ──────────────────
    client
        .batch_execute("ALTER TABLE project DROP COLUMN IF EXISTS old_project_id;")
        .await?;

    tracing::info!("V024b project ID migration complete (UUID → CHAR(14))");
    Ok(())
}

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

                 -- TlsProfileRun is created by the tester crate when TLS profiles
                 -- are used. Skip creation here to avoid FK conflicts with V025
                 -- project_id migration on fresh dashboard-only installs.

                 DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'testrun') THEN
                     ALTER TABLE testrun ADD COLUMN IF NOT EXISTS concurrency INTEGER NOT NULL DEFAULT 1;
                     ALTER TABLE testrun ADD COLUMN IF NOT EXISTS timeoutms BIGINT NOT NULL DEFAULT 30000;
                   END IF;
                 END $$;",
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

    // V016: Benchmark creation — VM catalog, config, cell tables
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 16", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V016 benchmark_creation migration...");
        client.batch_execute(V016_BENCHMARK_CREATION).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (16) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V016 migration complete");
    }

    // V017: Add cell_id/config_id to benchmark_run for cross-cell grouping
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 17", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V017 benchmark_run_cell_link migration...");
        client.batch_execute(V017_BENCHMARK_RUN_CELL_LINK).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (17) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V017 migration complete");
    }

    // V018: Scheduled benchmarks + regression detection
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 18", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V018 scheduled_benchmarks_regression migration...");
        client
            .batch_execute(V018_SCHEDULED_BENCHMARKS_REGRESSION)
            .await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (18) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V018 migration complete");
    }

    // V019: Drop FK constraints that block benchmark pipeline inserts.
    // The pipeline tables (benchmarkrun, benchmarksample) were created by the tester
    // with FKs to testrun/requestattempt, but benchmark pipeline bypasses those tables.
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 19", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V019 drop benchmark pipeline FK constraints...");
        client
            .batch_execute(
                "DO $$ BEGIN
                   IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmarkrun') THEN
                     ALTER TABLE benchmarkrun DROP CONSTRAINT IF EXISTS fk_benchmarkrun_testrun;
                   END IF;
                   IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmarksample') THEN
                     ALTER TABLE benchmarksample DROP CONSTRAINT IF EXISTS fk_benchmarksample_attempt;
                   END IF;
                 END $$;",
            )
            .await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (19) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V019 migration complete");
    }

    // V020: Rename benchmark_cell → benchmark_testbed, cell_id → testbed_id, add os column.
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 20", &[])
        .await?;

    if row.is_none() {
        let already_renamed: bool = client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmark_testbed')",
                &[],
            )
            .await?
            .get(0);

        if !already_renamed {
            tracing::info!("Applying V020 rename benchmark_cell to benchmark_testbed...");
            client
                .batch_execute(
                    "ALTER TABLE IF EXISTS benchmark_cell RENAME TO benchmark_testbed;
                     ALTER TABLE IF EXISTS benchmark_testbed RENAME COLUMN cell_id TO testbed_id;
                     ALTER TABLE IF EXISTS benchmark_run RENAME COLUMN cell_id TO testbed_id;
                     ALTER TABLE IF EXISTS benchmark_testbed ADD COLUMN IF NOT EXISTS os TEXT NOT NULL DEFAULT 'linux';
                     DROP INDEX IF EXISTS ix_benchmark_cell_config;
                     CREATE INDEX IF NOT EXISTS ix_benchmark_testbed_config ON benchmark_testbed (config_id);
                     DROP INDEX IF EXISTS ix_benchmark_run_cell;
                     CREATE INDEX IF NOT EXISTS ix_benchmark_run_testbed ON benchmark_run (testbed_id) WHERE testbed_id IS NOT NULL;",
                )
                .await?;
        } else {
            tracing::info!("V020: benchmark_testbed table already exists, skipping rename");
        }

        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (20) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V020 migration complete");
    }

    // V021: Per-request benchmark progress tracking
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 21", &[])
        .await?;

    if row.is_none() {
        let already_exists: bool = client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmark_request_progress')",
                &[],
            )
            .await?
            .get(0);

        if !already_exists {
            tracing::info!("Applying V021 benchmark_request_progress migration...");
            client
                .batch_execute(V021_BENCHMARK_REQUEST_PROGRESS)
                .await?;
        }

        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (21) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V021 migration complete");
    }

    // V022: Application benchmark mode — add benchmark_type, proxies, tester_os
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 22", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V022 application benchmark migration...");
        client.batch_execute(V022_APPLICATION_BENCHMARK).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (22) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V022 migration complete");
    }

    // V023: Performance log table
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 23", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V023 perf_log migration...");
        client.batch_execute(V023_PERF_LOG).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (23) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V023 migration complete");
    }

    // V024: Sovereignty zones + server registry
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 24", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V024 sovereignty_zones migration...");
        client.batch_execute(V024_SOVEREIGNTY_ZONES).await?;
        client.batch_execute(V024_SEED_ZONES).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (24) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V024 migration complete");
    }

    // V025: Convert project_id from UUID to 14-char base36
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 25", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V025 project ID migration (UUID → base36)...");
        migrate_project_ids(client).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (25) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;

        // Set default project ID from DB (the delete_protection=TRUE project)
        if let Ok(Some(row)) = client
            .query_opt(
                "SELECT project_id FROM project WHERE delete_protection = TRUE LIMIT 1",
                &[],
            )
            .await
        {
            let id: String = row.get("project_id");
            crate::auth::set_default_project_id(id);
        }

        tracing::info!("V025 migration complete");
    }

    // V026: System health table
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 26", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V026: system_health table...");
        client.batch_execute(V026_SYSTEM_HEALTH).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (26) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V026 migration complete");
    }

    // V027: Persistent testers
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 27", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V027: persistent testers schema...");
        client.batch_execute(V027_PERSISTENT_TESTERS).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (27) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V027 migration complete");
    }

    // V028: Dispatcher sweep partial index
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 28", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V028: dispatcher queued partial index...");
        client.batch_execute(V028_DISPATCHER_INDEX).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (28) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V028 migration complete");
    }

    // V029: Link project_tester to cloud_connection
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 29", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V029: link project_tester to cloud_connection...");
        client.batch_execute(V029_TESTER_CLOUD_CONN).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (29) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V029 migration complete");
    }

    // V030: Dynamic SSO providers, system config, member status
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 30", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V030: SSO providers + system config + member status...");
        client.batch_execute(V030_SSO_AND_MEMBER_STATUS).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (30) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V030 migration complete");
    }

    // V031: tester OS/arch info
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 31", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V031: tester OS + arch columns...");
        client.batch_execute(V031_TESTER_OS_INFO).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (31) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V031 migration complete");
    }

    // V032: link agent to project_tester so persistent testers surface as agents
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 32", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V032: agent.tester_id FK + index...");
        client.batch_execute(V032_AGENT_TESTER_LINK).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (32) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V032 migration complete");
    }

    // V033: agent_command table
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 33", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V033: agent_command table...");
        client.batch_execute(V033_AGENT_COMMAND).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (33) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V033 migration complete");
    }

    // V034: vm_lifecycle + cost_rate tables for VM usage history.
    // Design: docs/superpowers/specs/2026-04-15-vm-usage-history-design.md
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 34", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V034: vm_lifecycle + cost_rate tables...");
        client.batch_execute(V034_VM_LIFECYCLE).await?;
        client.batch_execute(V034_COST_RATE_SEED).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (34) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V034 migration complete");
    }

    // V035: Synthetic backfill of `created` events for existing testers.
    // Idempotent via WHERE NOT EXISTS — re-running a failed migration or
    // applying to a fresh DB that already has events skips rows we already
    // have. Only fires once, same pattern as every other V0xx migration.
    let row = client
        .query_opt("SELECT version FROM _migrations WHERE version = 35", &[])
        .await?;

    if row.is_none() {
        tracing::info!("Applying V035: backfill vm_lifecycle.created for existing testers...");
        client.batch_execute(V035_TESTER_CREATED_BACKFILL).await?;
        client
            .execute(
                "INSERT INTO _migrations (version) VALUES (35) ON CONFLICT DO NOTHING",
                &[],
            )
            .await?;
        tracing::info!("V035 migration complete");
    }

    Ok(())
}

/// V031: Add OS/arch/kernel info to project_tester so the UI can show what
/// distro/version the tester VM is running.
const V031_TESTER_OS_INFO: &str = r#"
ALTER TABLE project_tester
    ADD COLUMN IF NOT EXISTS requested_os       VARCHAR(50) DEFAULT 'ubuntu-24.04',
    ADD COLUMN IF NOT EXISTS requested_variant  VARCHAR(20) DEFAULT 'server',
    ADD COLUMN IF NOT EXISTS os_distro   VARCHAR(50),
    ADD COLUMN IF NOT EXISTS os_version  VARCHAR(50),
    ADD COLUMN IF NOT EXISTS os_variant  VARCHAR(20),
    ADD COLUMN IF NOT EXISTS os_arch     VARCHAR(20),
    ADD COLUMN IF NOT EXISTS os_kernel   VARCHAR(100);
"#;

/// V032: Link `agent` to `project_tester` so persistent testers surface as
/// agents when their installed networker-agent registers. Nullable so existing
/// standalone agents keep working.
const V032_AGENT_TESTER_LINK: &str = r#"
ALTER TABLE agent
    ADD COLUMN IF NOT EXISTS tester_id UUID
        REFERENCES project_tester(tester_id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_agent_tester ON agent(tester_id) WHERE tester_id IS NOT NULL;
"#;

/// V033: agent_command table. Tracks every dashboard->agent command
/// (dispatched, running, completed). Logs live in service_log with
/// service='agent-command' and the command_id in context.
const V033_AGENT_COMMAND: &str = r#"
CREATE TABLE IF NOT EXISTS agent_command (
  command_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  agent_id       UUID NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
  config_id      UUID,
  verb           TEXT NOT NULL,
  args           JSONB NOT NULL DEFAULT '{}'::jsonb,
  status         TEXT NOT NULL DEFAULT 'pending',
  result         JSONB,
  error_message  TEXT,
  created_by     UUID REFERENCES dash_user(user_id),
  created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  started_at     TIMESTAMPTZ,
  finished_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_agent_command_agent  ON agent_command(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_agent_command_config ON agent_command(config_id) WHERE config_id IS NOT NULL;
"#;

/// V034: VM usage history and cost tracking tables.
///
/// `vm_lifecycle` is an append-only log of every VM create/start/stop/delete
/// event. All cloud-connection-dependent columns are snapshotted as strings
/// so the history survives rename, soft-delete, or even hard-delete of the
/// source `cloud_connection` row. Design doc:
/// `docs/superpowers/specs/2026-04-15-vm-usage-history-design.md`.
///
/// `cost_rate` is a versioned per-cloud / per-vm-size rate table. Past
/// events price against the row whose `[effective_from, effective_to)`
/// window covers the event time, so rates can change going forward without
/// retroactively rewriting historical cost estimates.
const V034_VM_LIFECYCLE: &str = r#"
CREATE TABLE IF NOT EXISTS vm_lifecycle (
    event_id                     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id                   VARCHAR(20) NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,

    resource_type                TEXT NOT NULL,
    resource_id                  UUID NOT NULL,
    resource_name                TEXT,

    cloud                        TEXT NOT NULL,
    region                       TEXT,
    vm_size                      TEXT,
    vm_name                      TEXT,
    vm_resource_id               TEXT,

    cloud_connection_id          UUID,
    cloud_account_name_at_event  TEXT,
    provider_account_id          TEXT,

    event_type                   TEXT NOT NULL,
    event_time                   TIMESTAMPTZ NOT NULL,
    triggered_by                 UUID REFERENCES dash_user(user_id) ON DELETE SET NULL,
    metadata                     JSONB,

    created_at                   TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT vm_lifecycle_event_type_valid CHECK (
        event_type IN ('created', 'started', 'stopped', 'deleted', 'auto_shutdown', 'error')
    ),
    CONSTRAINT vm_lifecycle_resource_type_valid CHECK (
        resource_type IN ('tester', 'endpoint', 'benchmark')
    )
);

CREATE INDEX IF NOT EXISTS idx_vm_lifecycle_project_time
    ON vm_lifecycle(project_id, event_time DESC);
CREATE INDEX IF NOT EXISTS idx_vm_lifecycle_resource
    ON vm_lifecycle(resource_type, resource_id, event_time DESC);
CREATE INDEX IF NOT EXISTS idx_vm_lifecycle_account
    ON vm_lifecycle(provider_account_id, event_time DESC)
    WHERE provider_account_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS cost_rate (
    cost_rate_id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    cloud                TEXT NOT NULL,
    vm_size              TEXT NOT NULL,
    region               TEXT,
    rate_per_hour_usd    NUMERIC(12, 6) NOT NULL,
    effective_from       TIMESTAMPTZ NOT NULL,
    effective_to         TIMESTAMPTZ,
    source               TEXT NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    CONSTRAINT cost_rate_cloud_valid CHECK (cloud IN ('aws', 'azure', 'gcp')),
    CONSTRAINT cost_rate_rate_non_negative CHECK (rate_per_hour_usd >= 0),
    CONSTRAINT cost_rate_window_ordered CHECK (effective_to IS NULL OR effective_to > effective_from)
);

CREATE INDEX IF NOT EXISTS idx_cost_rate_lookup
    ON cost_rate(cloud, vm_size, region, effective_from DESC);
"#;

/// V034 seed: static USD-per-hour rates for the VM sizes we actually create
/// in the cloud providers today. Sourced from each cloud's public pricing
/// page as of 2026-04-15 (`source = 'static-v1'`). Region is left NULL for
/// a flat rate; regional overrides can be inserted later with a specific
/// `region` value and the lookup picks the more specific row first.
///
/// Kept deliberately small — just the sizes `cloud_provider.rs` actually
/// uses. Extend by adding rows with a newer `effective_from`; the old rows
/// keep pricing historical events.
const V034_COST_RATE_SEED: &str = r#"
INSERT INTO cost_rate (cloud, vm_size, region, rate_per_hour_usd, effective_from, source) VALUES
    -- AWS EC2 on-demand (us-east-1 baseline, flat across regions for v1)
    ('aws',   't3.nano',           NULL, 0.0052,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('aws',   't3.micro',          NULL, 0.0104,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('aws',   't3.small',          NULL, 0.0208,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('aws',   't3.medium',         NULL, 0.0416,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('aws',   't3.large',          NULL, 0.0832,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('aws',   'm5.large',          NULL, 0.0960,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('aws',   'm5.xlarge',         NULL, 0.1920,  '2026-04-15T00:00:00Z', 'static-v1'),

    -- Azure (eastus baseline, pay-as-you-go; Windows variants carry the
    -- per-core Windows license surcharge vs. the Linux list rate)
    ('azure', 'Standard_B2s',      NULL, 0.0416,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('azure', 'Standard_B2ms',     NULL, 0.0832,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('azure', 'Standard_D2s_v3',   NULL, 0.0960,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('azure', 'Standard_D4s_v3',   NULL, 0.1920,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('azure', 'Standard_D2s_v5',   NULL, 0.0960,  '2026-04-15T00:00:00Z', 'static-v1'),

    -- GCP Compute Engine (us-central1 baseline, sustained-use discount not
    -- applied; actual billing will reconcile to a lower effective rate)
    ('gcp',   'e2-micro',          NULL, 0.0094,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('gcp',   'e2-small',          NULL, 0.0188,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('gcp',   'e2-medium',         NULL, 0.0335,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('gcp',   'e2-standard-2',     NULL, 0.0670,  '2026-04-15T00:00:00Z', 'static-v1'),
    ('gcp',   'n2-standard-2',     NULL, 0.0971,  '2026-04-15T00:00:00Z', 'static-v1')
ON CONFLICT DO NOTHING;
"#;

/// V035: Backfill synthetic `created` events for testers that existed before
/// the runtime hooks landed. Without this the history table would be empty
/// of every VM created before v0.27.18 deploys, making the "total uptime"
/// column useless for the first week. Idempotent — skips any tester that
/// already has a `created` row.
///
/// Intentionally does NOT backfill `started` / `stopped` / `deleted`:
/// historical state transitions are unrecoverable (DB has only the current
/// snapshot), and synthesising them would pollute uptime math. Going
/// forward, real hooks capture the full picture; pre-existing rows just get
/// the single `created` anchor.
const V035_TESTER_CREATED_BACKFILL: &str = r#"
INSERT INTO vm_lifecycle (
    project_id, resource_type, resource_id, resource_name,
    cloud, region, vm_size, vm_name, vm_resource_id,
    cloud_connection_id,
    event_type, event_time, triggered_by, metadata
)
SELECT
    t.project_id, 'tester', t.tester_id, t.name,
    t.cloud, t.region, t.vm_size, t.vm_name, t.vm_resource_id,
    t.cloud_connection_id,
    'created', t.created_at, t.created_by,
    jsonb_build_object('source', 'v035-backfill')
FROM project_tester t
WHERE NOT EXISTS (
    SELECT 1 FROM vm_lifecycle v
    WHERE v.resource_type = 'tester'
      AND v.resource_id   = t.tester_id
      AND v.event_type    = 'created'
);
"#;
