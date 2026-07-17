BEGIN;
-- Step 1: Backfill email from username (BEFORE dropping username)
UPDATE dash_user SET email = username WHERE email IS NULL OR email = '';
-- Step 2: Enforce NOT NULL + UNIQUE on email
ALTER TABLE dash_user ALTER COLUMN email SET NOT NULL;
ALTER TABLE dash_user ADD CONSTRAINT dash_user_email_unique UNIQUE (email);
-- Step 3: Drop username
ALTER TABLE dash_user DROP COLUMN IF EXISTS username;
-- Step 4: Allow NULL password for SSO accounts
ALTER TABLE dash_user ALTER COLUMN password_hash DROP NOT NULL;
-- Step 5: Add new columns
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'pending';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS auth_provider VARCHAR(20) NOT NULL DEFAULT 'local';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_subject_id VARCHAR(255);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS display_name VARCHAR(200);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS avatar_url TEXT;
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_only BOOLEAN NOT NULL DEFAULT FALSE;
-- Step 6: Migrate disabled -> status
UPDATE dash_user SET status = 'active' WHERE disabled = FALSE;
UPDATE dash_user SET status = 'disabled' WHERE disabled = TRUE;
ALTER TABLE dash_user DROP COLUMN IF EXISTS disabled;
-- Step 7: Invalidate existing plaintext reset tokens
UPDATE dash_user SET password_reset_token = NULL, password_reset_expires = NULL;
-- Step 8: Indexes
CREATE INDEX IF NOT EXISTS ix_user_sso ON dash_user (auth_provider, sso_subject_id) WHERE sso_subject_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_user_status ON dash_user (status);
COMMIT;
