CREATE TABLE IF NOT EXISTS agent_command (
  command_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  agent_id       UUID NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
  config_id      UUID,
  verb           TEXT NOT NULL,
  args           JSONB NOT NULL DEFAULT '{}'::jsonb,
  status         TEXT NOT NULL DEFAULT 'pending',
  result         JSONB,
  error_message  TEXT,
  created_by     UUID REFERENCES dash_user(user_id),
  created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  started_at     TIMESTAMPTZ,
  finished_at    TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_agent_command_agent  ON agent_command(agent_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_agent_command_config ON agent_command(config_id) WHERE config_id IS NOT NULL;
