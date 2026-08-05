-- deadline-calibration.sql — re-derive the per-unit cell deadline from real runs.
--
-- Audit P2. ComparisonGroupsEndpoints.CellMaxDurationSecs budgets
--
--     max_duration_secs = runs * modes * SECONDS_PER_UNIT + 600
--
-- with SECONDS_PER_UNIT = 8. That 8 came from ONE live observation
-- (2026-08-04): at 4s per unit the slower proxies were killed at 78-85%
-- complete. It is a reasonable number derived from a real failure — but it
-- lives in a code comment, so nobody can check whether it still holds without
-- repeating the incident.
--
-- This query recomputes the distribution from completed runs so the constant
-- can be re-derived instead of remembered. Run it against the control-plane
-- database:
--
--     psql "$DASHBOARD_DB_URL" -f scripts/deadline-calibration.sql
--
-- Read the p95 (and max) of seconds_per_unit. The constant should sit ABOVE
-- p95 with margin: a deadline that merely matches the typical run kills the
-- slow tail, which is exactly the failure this replaced.
--
-- Runs that were themselves killed by a deadline are excluded — including them
-- would measure the old limit rather than the work, and each recalibration
-- would ratchet the number downward toward the very failure it is meant to
-- prevent.

WITH sized AS (
    SELECT
        r.id                                        AS run_id,
        r.project_id,
        c.name                                      AS config_name,
        EXTRACT(EPOCH FROM (r.finished_at - r.started_at)) AS elapsed_secs,
        COALESCE(NULLIF((c.workload -> 'runs')::text, 'null')::int, 10) AS runs,
        GREATEST(COALESCE(jsonb_array_length(c.workload -> 'modes'), 1), 1)  AS modes,
        r.success_count + r.failure_count           AS attempts
    FROM test_run r
    JOIN test_config c ON c.id = r.test_config_id
    WHERE r.status = 'completed'
      AND r.started_at IS NOT NULL
      AND r.finished_at IS NOT NULL
      -- A run stopped by its own deadline measures the limit, not the work.
      AND COALESCE(r.error_message, '') NOT ILIKE '%deadline%'
      AND COALESCE(r.error_message, '') NOT ILIKE '%timed out%'
      AND r.created_at > NOW() - INTERVAL '90 days'
),
per_unit AS (
    SELECT
        run_id, config_name, elapsed_secs, runs, modes, attempts,
        runs * modes                        AS units,
        elapsed_secs / NULLIF(runs * modes, 0) AS seconds_per_unit,
        -- The comment claims real attempts run ~1.7x runs*modes because payload
        -- sizes multiply the throughput modes. Surfaced so that claim is
        -- checkable too, not just the timing.
        attempts::numeric / NULLIF(runs * modes, 0) AS attempt_multiplier
    FROM sized
    WHERE runs * modes > 0
      AND elapsed_secs > 0
)
SELECT
    COUNT(*)                                                          AS sample_runs,
    ROUND(MIN(seconds_per_unit)::numeric, 2)                          AS min_s_per_unit,
    ROUND(PERCENTILE_CONT(0.50) WITHIN GROUP (ORDER BY seconds_per_unit)::numeric, 2) AS p50_s_per_unit,
    ROUND(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY seconds_per_unit)::numeric, 2) AS p95_s_per_unit,
    ROUND(MAX(seconds_per_unit)::numeric, 2)                          AS max_s_per_unit,
    ROUND(PERCENTILE_CONT(0.95) WITHIN GROUP (ORDER BY attempt_multiplier)::numeric, 2) AS p95_attempt_multiplier
FROM per_unit;

-- The slowest individual runs — this is the tail the deadline has to clear.
WITH sized AS (
    SELECT
        r.id AS run_id,
        c.name AS config_name,
        EXTRACT(EPOCH FROM (r.finished_at - r.started_at)) AS elapsed_secs,
        COALESCE(NULLIF((c.workload -> 'runs')::text, 'null')::int, 10) AS runs,
        GREATEST(COALESCE(jsonb_array_length(c.workload -> 'modes'), 1), 1) AS modes
    FROM test_run r
    JOIN test_config c ON c.id = r.test_config_id
    WHERE r.status = 'completed'
      AND r.started_at IS NOT NULL
      AND r.finished_at IS NOT NULL
      AND r.created_at > NOW() - INTERVAL '90 days'
)
SELECT
    run_id,
    config_name,
    runs,
    modes,
    ROUND(elapsed_secs::numeric, 0) AS elapsed_secs,
    ROUND((elapsed_secs / NULLIF(runs * modes, 0))::numeric, 2) AS seconds_per_unit
FROM sized
WHERE runs * modes > 0 AND elapsed_secs > 0
ORDER BY seconds_per_unit DESC
LIMIT 15;
