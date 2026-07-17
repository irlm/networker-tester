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
