CREATE TABLE IF NOT EXISTS benchmark_compare_preset (
    preset_id        UUID           NOT NULL PRIMARY KEY,
    project_id       UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    created_by       UUID           NOT NULL REFERENCES dash_user(user_id),
    name             VARCHAR(200)   NOT NULL,
    name_key         VARCHAR(200)   NOT NULL,
    run_ids          UUID[]         NOT NULL,
    baseline_run_id  UUID           NOT NULL,
    target_search    VARCHAR(200)   NOT NULL DEFAULT '',
    scenario         VARCHAR(100)   NOT NULL DEFAULT '',
    phase_model      VARCHAR(200)   NOT NULL DEFAULT '',
    server_region    VARCHAR(100)   NOT NULL DEFAULT '',
    network_type     VARCHAR(50)    NOT NULL DEFAULT '',
    created_at       TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_benchmark_compare_preset_name
    ON benchmark_compare_preset (project_id, name_key);
CREATE INDEX IF NOT EXISTS ix_benchmark_compare_preset_project_updated
    ON benchmark_compare_preset (project_id, updated_at DESC);
