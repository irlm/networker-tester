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
