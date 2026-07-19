-- V041: alerting — threshold rules, notification channels, alert events.
--
-- Closes the monitoring loop: schedules run probes forever, and until now
-- nothing told anyone when a metric regressed. A rule watches one metric of
-- one test_config (or every config in the project when test_config_id is
-- NULL), compares it against a threshold on every terminal run, and fires
-- through a notification channel (webhook or email) when the last
-- window_runs consecutive runs all breach. Events record each state
-- transition (firing/resolved) plus the delivery outcome.

CREATE TABLE IF NOT EXISTS alert_channel (
    channel_id  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('webhook','email')),
    name        TEXT NOT NULL,
    -- webhook: {"url": "...", "secret": "..."} (secret optional, enables the
    -- HMAC-SHA256 signature header). email: {"to": ["a@example.com", ...]}.
    config      JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_alert_channel_project ON alert_channel(project_id);

CREATE TABLE IF NOT EXISTS alert_rule (
    rule_id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    -- NULL = the rule applies to every test_config in the project.
    test_config_id  UUID REFERENCES test_config(id) ON DELETE CASCADE,
    metric          TEXT NOT NULL CHECK (metric IN ('p95_ms','mean_ms','error_rate','success_rate')),
    comparator      TEXT NOT NULL CHECK (comparator IN ('gt','lt')),
    threshold       DOUBLE PRECISION NOT NULL,
    -- Consecutive terminal runs that must breach before the rule fires.
    window_runs     INT NOT NULL DEFAULT 1 CHECK (window_runs BETWEEN 1 AND 50),
    enabled         BOOLEAN NOT NULL DEFAULT TRUE,
    -- No cascade: deleting a channel that rules still reference is rejected
    -- (the API pre-checks and returns 409) so rules are never dropped silently.
    channel_id      UUID NOT NULL REFERENCES alert_channel(channel_id),
    created_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_alert_rule_project ON alert_rule(project_id);
CREATE INDEX IF NOT EXISTS ix_alert_rule_config ON alert_rule(test_config_id) WHERE test_config_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS alert_event (
    event_id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_id         UUID NOT NULL REFERENCES alert_rule(rule_id) ON DELETE CASCADE,
    run_id          UUID NOT NULL REFERENCES test_run(id) ON DELETE CASCADE,
    fired_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    state           TEXT NOT NULL CHECK (state IN ('firing','resolved')),
    value           DOUBLE PRECISION,
    message         TEXT,
    -- 'pending' → 'delivered' | 'failed: ...' | 'skipped: ...'.
    delivery_status TEXT
);
CREATE INDEX IF NOT EXISTS ix_alert_event_rule ON alert_event(rule_id, fired_at DESC);
CREATE INDEX IF NOT EXISTS ix_alert_event_run ON alert_event(run_id);
