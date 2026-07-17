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
