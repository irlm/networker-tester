-- V051: in-product canary dispatch history.
--
-- The prod run-execution canary (.github/workflows/soak-canary.yml) can be
-- triggered from the admin UI (CanaryEndpoints). GitHub's workflow_dispatch API
-- answers 204 with NO run id, and the Actions UI is the only place the result
-- shows up — so an admin who triggers a run cannot later tell, from the product,
-- WHEN they asked for it or HOW it ended, and loses that entirely if GitHub is
-- unreachable or they are looking at a different deployment.
--
-- This table records each in-product dispatch at trigger time (who/when/ref/
-- inputs). CanaryRunPoller fills run_id/run_url/run_status/conclusion in later,
-- once the run appears and completes — so the history is durable in OUR database
-- independent of GitHub availability.
--
-- Idempotent (CREATE TABLE IF NOT EXISTS); no dependency on other tables.
CREATE TABLE IF NOT EXISTS canary_dispatch (
    id            UUID        PRIMARY KEY,
    requested_by  TEXT,
    requested_at  TIMESTAMPTZ NOT NULL,
    git_ref       TEXT        NOT NULL,
    inputs        JSONB       NOT NULL DEFAULT '{}'::jsonb,
    run_id        BIGINT,
    run_url       TEXT,
    run_status    TEXT,
    conclusion    TEXT,
    updated_at    TIMESTAMPTZ NOT NULL
);

-- Newest-first listing, and the poller's "unresolved / not yet completed" scan.
CREATE INDEX IF NOT EXISTS ix_canary_dispatch_requested_at
    ON canary_dispatch (requested_at DESC);
