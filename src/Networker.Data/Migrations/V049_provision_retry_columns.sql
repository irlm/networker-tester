-- V049: test_run quota-retry bookkeeping.
--
-- Matrix cells that failed provisioning on a cloud CAPACITY quota (Azure
-- regional cores / public IPs) used to fail PERMANENTLY — but quota pressure
-- is transient by construction: each finished cell's teardown frees its
-- cores/IP, so the same kick minutes later succeeds (user-caught 2026-08-13:
-- 13 of 15 comparison-matrix cells died on 'exceeding approved cores quota'
-- while two that raced in later completed fine).
--
-- provision_attempts        — kicks consumed; the orchestrator caps retries.
-- next_provision_attempt_at — backoff: the kick pass skips the run until this
--                             passes. NULL = immediately eligible (default,
--                             and the value for every pre-V049 row).
--
-- Idempotent (ADD COLUMN IF NOT EXISTS); no backfill — historical failed runs
-- stay failed.
ALTER TABLE test_run
    ADD COLUMN IF NOT EXISTS provision_attempts SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS next_provision_attempt_at TIMESTAMPTZ;
