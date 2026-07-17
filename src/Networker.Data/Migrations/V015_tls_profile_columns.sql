ALTER TABLE job ADD COLUMN IF NOT EXISTS tls_profile_run_id UUID;

-- TlsProfileRun is created by the tester crate when TLS profiles
-- are used. Skip creation here to avoid FK conflicts with V025
-- project_id migration on fresh dashboard-only installs.

DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'testrun') THEN
    ALTER TABLE testrun ADD COLUMN IF NOT EXISTS concurrency INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE testrun ADD COLUMN IF NOT EXISTS timeoutms BIGINT NOT NULL DEFAULT 30000;
  END IF;
END $$;
