ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS password_reset_token VARCHAR(128);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS password_reset_expires TIMESTAMPTZ;
