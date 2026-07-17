-- Broaden endpoint_kind CHECK (Postgres doesn't support ALTER CONSTRAINT for CHECKs).
ALTER TABLE test_config DROP CONSTRAINT IF EXISTS test_config_endpoint_kind_check;
ALTER TABLE test_config
    ADD CONSTRAINT test_config_endpoint_kind_check
    CHECK (endpoint_kind IN ('network','proxy','runtime','pending'));

-- Broaden test_run.status CHECK to allow 'provisioning'.
ALTER TABLE test_run DROP CONSTRAINT IF EXISTS test_run_status_check;
ALTER TABLE test_run
    ADD CONSTRAINT test_run_status_check
    CHECK (status IN ('queued','provisioning','running','completed','failed','cancelled'));

-- Link a provisioning run to the deployment that will satisfy it.
ALTER TABLE test_run
    ADD COLUMN IF NOT EXISTS provisioning_deployment_id UUID
    REFERENCES deployment(deployment_id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS ix_test_run_provisioning
    ON test_run(provisioning_deployment_id)
    WHERE provisioning_deployment_id IS NOT NULL;
