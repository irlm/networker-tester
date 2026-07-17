CREATE TABLE IF NOT EXISTS comparison_group (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      TEXT NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    base_workload   JSONB NOT NULL,
    methodology     JSONB,
    cells           JSONB NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','running','completed','failed')),
    created_by      UUID,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_comparison_group_project ON comparison_group(project_id);

ALTER TABLE test_run ADD COLUMN IF NOT EXISTS comparison_group_id UUID REFERENCES comparison_group(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS ix_test_run_comparison ON test_run(comparison_group_id) WHERE comparison_group_id IS NOT NULL;
