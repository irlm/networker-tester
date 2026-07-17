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
