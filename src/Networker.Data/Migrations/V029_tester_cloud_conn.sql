-- V029: Link testers to cloud_connections for secretless provisioning.
ALTER TABLE project_tester
  ADD COLUMN IF NOT EXISTS cloud_connection_id UUID
    REFERENCES cloud_connection(connection_id) ON DELETE RESTRICT;

CREATE INDEX IF NOT EXISTS idx_project_tester_cloud_conn
  ON project_tester(cloud_connection_id)
  WHERE cloud_connection_id IS NOT NULL;
