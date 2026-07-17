-- The Rust runner checked for benchmark_testbed before renaming; the same
-- guard is expressed here as a DO block so re-application is a no-op.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'benchmark_testbed') THEN
        ALTER TABLE IF EXISTS benchmark_cell RENAME TO benchmark_testbed;
        ALTER TABLE IF EXISTS benchmark_testbed RENAME COLUMN cell_id TO testbed_id;
        ALTER TABLE IF EXISTS benchmark_run RENAME COLUMN cell_id TO testbed_id;
        ALTER TABLE IF EXISTS benchmark_testbed ADD COLUMN IF NOT EXISTS os TEXT NOT NULL DEFAULT 'linux';
        DROP INDEX IF EXISTS ix_benchmark_cell_config;
        CREATE INDEX IF NOT EXISTS ix_benchmark_testbed_config ON benchmark_testbed (config_id);
        DROP INDEX IF EXISTS ix_benchmark_run_cell;
        CREATE INDEX IF NOT EXISTS ix_benchmark_run_testbed ON benchmark_run (testbed_id) WHERE testbed_id IS NOT NULL;
    END IF;
END $$;
