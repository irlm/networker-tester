ALTER TABLE project_tester
    ADD COLUMN IF NOT EXISTS cloud_account_id UUID
        REFERENCES cloud_account(account_id) ON DELETE SET NULL;
