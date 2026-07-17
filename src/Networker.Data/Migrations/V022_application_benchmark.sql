ALTER TABLE benchmark_config
    ADD COLUMN IF NOT EXISTS benchmark_type TEXT NOT NULL DEFAULT 'fullstack';

ALTER TABLE benchmark_testbed
    ADD COLUMN IF NOT EXISTS proxies JSONB NOT NULL DEFAULT '[]'::jsonb,
    ADD COLUMN IF NOT EXISTS tester_os TEXT NOT NULL DEFAULT 'server';

CREATE INDEX IF NOT EXISTS ix_benchmark_config_type
    ON benchmark_config (benchmark_type);
