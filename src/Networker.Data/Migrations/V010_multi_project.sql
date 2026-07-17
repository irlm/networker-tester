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
