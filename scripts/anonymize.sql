-- anonymize.sql — Scrub sensitive data for sandbox environments.
-- Run against a COPY of networker_core, never against production.
--
-- Usage:
--   psql "$SANDBOX_DB_URL" -f scripts/anonymize.sql

BEGIN;

-- Anonymize user identities
UPDATE dash_user SET
    email = 'user_' || user_id::text || '@sandbox.local',
    display_name = 'User ' || LEFT(user_id::text, 8),
    password_hash = '$argon2id$v=19$m=19456,t=2,p=1$sandbox$sandbox',
    avatar_url = NULL,
    password_reset_token = NULL,
    password_reset_expires = NULL;

-- Delete cloud credentials (never copy real creds to sandbox)
DELETE FROM cloud_account;
DELETE FROM cloud_connection;

-- Delete access tokens and invites
DELETE FROM workspace_invite;
DELETE FROM share_link;
DELETE FROM command_approval;

COMMIT;
