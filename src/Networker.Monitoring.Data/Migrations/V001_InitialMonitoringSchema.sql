CREATE TABLE monitor (
    monitor_id UUID PRIMARY KEY,
    project_id VARCHAR(128) NOT NULL,
    name VARCHAR(200) NOT NULL,
    target_url VARCHAR(2048) NOT NULL,
    method VARCHAR(8) NOT NULL,
    assertion_config JSONB NOT NULL DEFAULT '{}'::jsonb,
    interval_seconds INT NOT NULL DEFAULT 60,
    timeout_ms INT NOT NULL DEFAULT 10000,
    enabled BOOLEAN NOT NULL DEFAULT true,
    criticality VARCHAR(16) NOT NULL DEFAULT 'reporting',
    retention_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    next_check_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_owner VARCHAR(200),
    lease_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT monitor_method_chk CHECK (method IN ('GET', 'HEAD')),
    CONSTRAINT monitor_interval_chk CHECK (interval_seconds BETWEEN 60 AND 86400),
    CONSTRAINT monitor_timeout_chk CHECK (timeout_ms BETWEEN 100 AND 30000),
    CONSTRAINT monitor_criticality_chk CHECK (criticality IN ('reporting', 'critical'))
);

CREATE UNIQUE INDEX monitor_project_name_uq
    ON monitor (project_id, name)
    WHERE deleted_at IS NULL;
CREATE INDEX monitor_due_idx
    ON monitor (enabled, next_check_at)
    WHERE deleted_at IS NULL;

CREATE TABLE monitor_location (
    location_id UUID PRIMARY KEY,
    name VARCHAR(200) NOT NULL UNIQUE,
    provider VARCHAR(100),
    region VARCHAR(100),
    last_heartbeat_at TIMESTAMPTZ,
    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE monitor_location_assignment (
    monitor_id UUID NOT NULL REFERENCES monitor(monitor_id) ON DELETE CASCADE,
    location_id UUID NOT NULL REFERENCES monitor_location(location_id) ON DELETE RESTRICT,
    participates_in_incident_quorum BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (monitor_id, location_id)
);

-- V1 intentionally enforces one active assignment. Dropping this index is the
-- explicit schema switch when multi-location execution becomes a product feature.
CREATE UNIQUE INDEX monitor_single_location_uq
    ON monitor_location_assignment (monitor_id);

CREATE TABLE monitor_check (
    check_id UUID PRIMARY KEY,
    monitor_id UUID NOT NULL REFERENCES monitor(monitor_id) ON DELETE CASCADE,
    location_id UUID NOT NULL REFERENCES monitor_location(location_id) ON DELETE RESTRICT,
    scheduled_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    outcome VARCHAR(16) NOT NULL,
    failure_kind VARCHAR(32),
    status_code INT,
    dns_ms INT,
    connect_ms INT,
    tls_ms INT,
    ttfb_ms INT,
    total_ms INT,
    assertion_results JSONB NOT NULL DEFAULT '{}'::jsonb,
    response_digest VARCHAR(128),
    error_summary VARCHAR(1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT monitor_check_outcome_chk
        CHECK (outcome IN ('healthy', 'warning', 'critical', 'unknown'))
);

CREATE INDEX monitor_check_history_idx
    ON monitor_check (monitor_id, scheduled_at DESC);

INSERT INTO monitor_location (
    location_id, name, provider, region, last_heartbeat_at, enabled
) VALUES (
    '00000000-0000-4000-8000-000000000001',
    'default',
    'self-hosted',
    'local',
    now(),
    true
);
