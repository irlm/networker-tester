-- Allow schedules to reference a benchmark config template
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS benchmark_config_id UUID REFERENCES benchmark_config(config_id);

-- Regression detection results
CREATE TABLE IF NOT EXISTS benchmark_regression (
    regression_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    config_id UUID NOT NULL REFERENCES benchmark_config(config_id) ON DELETE CASCADE,
    baseline_config_id UUID REFERENCES benchmark_config(config_id),
    language VARCHAR(100) NOT NULL,
    metric VARCHAR(50) NOT NULL,
    baseline_value DOUBLE PRECISION NOT NULL,
    current_value DOUBLE PRECISION NOT NULL,
    delta_percent DOUBLE PRECISION NOT NULL,
    severity VARCHAR(20) NOT NULL DEFAULT 'warning',
    detected_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_benchmark_regression_config ON benchmark_regression (config_id, detected_at DESC);
CREATE INDEX IF NOT EXISTS ix_benchmark_regression_project ON benchmark_regression (config_id);
