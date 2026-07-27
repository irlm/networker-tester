-- V047: benchmark_regression — regression detection results on the unified
-- test_config/test_run schema.
--
-- The pre-unification benchmark_regression table (V018) hung off the old
-- benchmark_config table and was dropped by V036; the detection logic itself
-- was never ported (RegressionAnalyzer was a documented stub — deep-measurement
-- audit M5/A12/G2). This recreates the store keyed on the unified schema:
-- one row per (completed run, case, breached metric), written by
-- BenchmarkRegressionDetector when a benchmark run's per-case p50 worsens by
-- more than 10% vs the baseline run, or its success rate drops below 99%.
--
-- baseline_run_id is SET NULL on delete so pruning old runs keeps the
-- detection record; test_run_id/test_config_id cascade with their parents.
CREATE TABLE IF NOT EXISTS benchmark_regression (
    regression_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    test_config_id  UUID NOT NULL REFERENCES test_config(id) ON DELETE CASCADE,
    test_run_id     UUID NOT NULL REFERENCES test_run(id) ON DELETE CASCADE,
    baseline_run_id UUID REFERENCES test_run(id) ON DELETE SET NULL,
    case_id         TEXT NOT NULL,
    metric          TEXT NOT NULL,
    metric_unit     TEXT NOT NULL,
    baseline_value  DOUBLE PRECISION NOT NULL,
    current_value   DOUBLE PRECISION NOT NULL,
    delta_percent   DOUBLE PRECISION NOT NULL,
    severity        TEXT NOT NULL DEFAULT 'warning',
    detected_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_benchmark_regression_config
    ON benchmark_regression (test_config_id, detected_at DESC);
CREATE INDEX IF NOT EXISTS ix_benchmark_regression_run
    ON benchmark_regression (test_run_id);
