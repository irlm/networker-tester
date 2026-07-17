ALTER TABLE benchmark_run ADD COLUMN IF NOT EXISTS cell_id UUID;
ALTER TABLE benchmark_run ADD COLUMN IF NOT EXISTS config_id UUID;
CREATE INDEX IF NOT EXISTS ix_benchmark_run_config ON benchmark_run (config_id) WHERE config_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_benchmark_run_cell ON benchmark_run (cell_id) WHERE cell_id IS NOT NULL;
