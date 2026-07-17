-- ── 1. DROP old tables in FK-safe order ──────────────────────────────────
-- Drop schedule first (FKs into job/deployment/benchmark_config).
DROP TABLE IF EXISTS schedule CASCADE;

-- Drop benchmark-* tables that hang off benchmark_config.
DROP TABLE IF EXISTS benchmark_cell CASCADE;
DROP TABLE IF EXISTS benchmark_regression CASCADE;
DROP TABLE IF EXISTS benchmark_request_progress CASCADE;
DROP TABLE IF EXISTS benchmark_compare_preset CASCADE;
DROP TABLE IF EXISTS benchmark_result CASCADE;
DROP TABLE IF EXISTS benchmark_run CASCADE;

-- Drop the config tables themselves.
DROP TABLE IF EXISTS benchmark_config_preset CASCADE;
DROP TABLE IF EXISTS benchmark_config CASCADE;

-- job_config was never a separate table in this repo, but guard anyway.
DROP TABLE IF EXISTS job_config CASCADE;
DROP TABLE IF EXISTS job CASCADE;

-- test_definition was only referenced by job/schedule — now orphaned.
DROP TABLE IF EXISTS test_definition CASCADE;

-- ── 2. CREATE new unified tables ─────────────────────────────────────────
-- test_config: the canonical definition of a test (simple OR benchmark).
-- `methodology IS NOT NULL` flips benchmark mode on.
CREATE TABLE IF NOT EXISTS test_config (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id         TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    name               TEXT NOT NULL,
    description        TEXT,

    endpoint_kind      TEXT NOT NULL CHECK (endpoint_kind IN ('network','proxy','runtime')),
    endpoint_ref       JSONB NOT NULL,

    workload           JSONB NOT NULL,
    methodology        JSONB,

    created_by         UUID REFERENCES dash_user(user_id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- baseline_run_id FK is added after test_run exists (circular).
    baseline_run_id    UUID,

    max_duration_secs  INT NOT NULL DEFAULT 900,

    UNIQUE (project_id, name)
);

-- test_run: every execution of a test_config produces one row here.
CREATE TABLE IF NOT EXISTS test_run (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_config_id     UUID NOT NULL REFERENCES test_config(id) ON DELETE CASCADE,
    project_id         TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,

    status             TEXT NOT NULL CHECK (status IN ('queued','running','completed','failed','cancelled')),
    started_at         TIMESTAMPTZ,
    finished_at        TIMESTAMPTZ,

    success_count      INT NOT NULL DEFAULT 0,
    failure_count      INT NOT NULL DEFAULT 0,
    error_message      TEXT,

    -- artifact_id FK added after benchmark_artifact exists (circular).
    artifact_id        UUID,

    tester_id          UUID REFERENCES project_tester(tester_id) ON DELETE SET NULL,
    worker_id          TEXT,
    last_heartbeat     TIMESTAMPTZ,

    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Now wire up the circular FKs on test_config.baseline_run_id and
-- test_run.artifact_id once both tables exist.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'test_config_baseline_run_fk'
    ) THEN
        ALTER TABLE test_config
            ADD CONSTRAINT test_config_baseline_run_fk
            FOREIGN KEY (baseline_run_id) REFERENCES test_run(id) ON DELETE SET NULL;
    END IF;
END$$;

-- benchmark_artifact: methodology-mode rich artifact. Only inserted when
-- test_config.methodology IS NOT NULL.
CREATE TABLE IF NOT EXISTS benchmark_artifact (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_run_id        UUID NOT NULL REFERENCES test_run(id) ON DELETE CASCADE,

    environment        JSONB NOT NULL,
    methodology        JSONB NOT NULL,
    launches           JSONB NOT NULL,
    cases              JSONB NOT NULL,
    samples            JSONB,
    summaries          JSONB NOT NULL,
    data_quality       JSONB NOT NULL,

    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'test_run_artifact_fk'
    ) THEN
        ALTER TABLE test_run
            ADD CONSTRAINT test_run_artifact_fk
            FOREIGN KEY (artifact_id) REFERENCES benchmark_artifact(id) ON DELETE SET NULL;
    END IF;
END$$;

-- test_schedule: unified schedule keyed on test_config_id only.
CREATE TABLE IF NOT EXISTS test_schedule (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_config_id     UUID NOT NULL REFERENCES test_config(id) ON DELETE CASCADE,
    project_id         TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,

    cron_expr          TEXT NOT NULL,
    timezone           TEXT NOT NULL DEFAULT 'UTC',
    enabled            BOOLEAN NOT NULL DEFAULT TRUE,

    last_fired_at      TIMESTAMPTZ,
    last_run_id        UUID REFERENCES test_run(id) ON DELETE SET NULL,
    next_fire_at       TIMESTAMPTZ,

    created_by         UUID REFERENCES dash_user(user_id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Indexes ────────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS ix_test_config_project       ON test_config(project_id);
CREATE INDEX IF NOT EXISTS ix_test_config_endpoint_kind ON test_config(endpoint_kind);
CREATE INDEX IF NOT EXISTS ix_test_config_is_benchmark  ON test_config((methodology IS NOT NULL));
CREATE INDEX IF NOT EXISTS ix_test_run_config           ON test_run(test_config_id);
CREATE INDEX IF NOT EXISTS ix_test_run_project_status   ON test_run(project_id, status);
CREATE INDEX IF NOT EXISTS ix_test_run_created          ON test_run(created_at DESC);
CREATE INDEX IF NOT EXISTS ix_benchmark_artifact_run    ON benchmark_artifact(test_run_id);
CREATE INDEX IF NOT EXISTS ix_test_schedule_enabled_next ON test_schedule(enabled, next_fire_at);
CREATE INDEX IF NOT EXISTS ix_test_schedule_config      ON test_schedule(test_config_id);

-- ── 3. Per-protocol phase detail tables: rename RunId → test_run_id ──────
-- Created by networker-tester's V001 init (mixed-case identifiers). Rename
-- only — the column keeps its old FK target until networker-tester updates
-- its DDL in the same PR (Agent C scope). We drop the old FK here since
-- the old TestRun parent table contract is now owned by the dashboard.
--
-- Guard with IF EXISTS on every step: a fresh dev DB without V001 data
-- (no TestRun table yet) must not break the migration.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'dnsresult') THEN
        ALTER TABLE DnsResult RENAME COLUMN RunId TO test_run_id;
    END IF;
EXCEPTION WHEN undefined_column THEN NULL;
END$$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'tcpresult') THEN
        ALTER TABLE TcpResult RENAME COLUMN RunId TO test_run_id;
    END IF;
EXCEPTION WHEN undefined_column THEN NULL;
END$$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'tlsresult') THEN
        ALTER TABLE TlsResult RENAME COLUMN RunId TO test_run_id;
    END IF;
EXCEPTION WHEN undefined_column THEN NULL;
END$$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'httpresult') THEN
        ALTER TABLE HttpResult RENAME COLUMN RunId TO test_run_id;
    END IF;
EXCEPTION WHEN undefined_column THEN NULL;
END$$;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'udpresult') THEN
        ALTER TABLE UdpResult RENAME COLUMN RunId TO test_run_id;
    END IF;
EXCEPTION WHEN undefined_column THEN NULL;
END$$;
