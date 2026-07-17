CREATE INDEX IF NOT EXISTS idx_benchmark_config_queued
    ON benchmark_config (tester_id, queued_at NULLS LAST)
    WHERE status = 'queued';
