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
