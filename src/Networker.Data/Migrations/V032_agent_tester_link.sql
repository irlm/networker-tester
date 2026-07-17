ALTER TABLE agent
    ADD COLUMN IF NOT EXISTS tester_id UUID
        REFERENCES project_tester(tester_id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_agent_tester ON agent(tester_id) WHERE tester_id IS NOT NULL;
