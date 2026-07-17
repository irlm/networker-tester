-- V012: Workspace management — invites, warnings, soft-delete

CREATE TABLE IF NOT EXISTS workspace_invite (
    invite_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id  UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    email       VARCHAR(255) NOT NULL,
    role        VARCHAR(20) NOT NULL DEFAULT 'viewer',
    token_hash  VARCHAR(128) NOT NULL,
    status      VARCHAR(20) NOT NULL DEFAULT 'pending',
    invited_by  UUID NOT NULL REFERENCES dash_user(user_id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    accepted_by UUID REFERENCES dash_user(user_id)
);
CREATE INDEX IF NOT EXISTS ix_workspace_invite_token ON workspace_invite (token_hash);
CREATE INDEX IF NOT EXISTS ix_workspace_invite_project ON workspace_invite (project_id, status);

CREATE TABLE IF NOT EXISTS workspace_warning (
    warning_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id   UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    warning_type VARCHAR(30) NOT NULL,
    sent_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_workspace_warning_unique ON workspace_warning (project_id, warning_type);

ALTER TABLE project ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;
ALTER TABLE project ADD COLUMN IF NOT EXISTS delete_protection BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE project SET delete_protection = TRUE WHERE project_id = '00000000-0000-0000-0000-000000000001';
