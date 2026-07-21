-- V043: LagHound SDK-endpoint probe token, encrypted at rest.
--
-- Wave 2 of the LagHound SDK feature. An "SDK endpoint" is a test_config with
-- mode 'sdkprobe' that points at a customer URL exposing the LagHound SDK
-- routes (docs/sdk/contract-v1.md). The probe authenticates with a per-endpoint
-- shared secret sent as the X-LagHound-Token header. That secret must never be
-- stored in plaintext, so it is encrypted with the SAME AES-256-GCM scheme the
-- cloud-account credentials use (Networker.Security.CredentialCipher):
-- ciphertext-with-tag + a 12-byte nonce in two separate bytea columns, exactly
-- like cloud_account.credentials_enc / credentials_nonce.
--
-- Both columns are nullable: a config that is not an sdkprobe endpoint (or an
-- sdkprobe endpoint whose route needs no token) simply has NULL token columns.
-- Fully idempotent (ADD COLUMN IF NOT EXISTS) so re-running the chain no-ops.

ALTER TABLE test_config
    ADD COLUMN IF NOT EXISTS token_enc   BYTEA,
    ADD COLUMN IF NOT EXISTS token_nonce BYTEA;
