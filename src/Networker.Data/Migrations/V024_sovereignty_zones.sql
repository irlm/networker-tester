CREATE TABLE IF NOT EXISTS sovereignty_zone (
    code              CHAR(2)       NOT NULL PRIMARY KEY,
    parent_code       CHAR(2),
    name              VARCHAR(50)   NOT NULL UNIQUE,
    display           VARCHAR(100)  NOT NULL,
    legal_note        VARCHAR(255),
    compliance_level  VARCHAR(100),
    fallback_zone     CHAR(2),
    auto_detect       JSONB         NOT NULL DEFAULT '{}',
    requires_approval BOOLEAN       NOT NULL DEFAULT FALSE,
    requires_mfa      BOOLEAN       NOT NULL DEFAULT FALSE,
    status            VARCHAR(20)   NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ   NOT NULL DEFAULT now(),
    FOREIGN KEY (parent_code) REFERENCES sovereignty_zone(code),
    FOREIGN KEY (fallback_zone) REFERENCES sovereignty_zone(code)
);

CREATE TABLE IF NOT EXISTS server_registry (
    server_id   CHAR(3)       NOT NULL,
    zone_code   CHAR(2)       NOT NULL,
    hostname    VARCHAR(255)  NOT NULL,
    endpoint    VARCHAR(255)  NOT NULL,
    internal_ip VARCHAR(45),
    db_url      VARCHAR(500),
    status      VARCHAR(20)   NOT NULL DEFAULT 'active',
    last_health TIMESTAMPTZ,
    priority    INT           NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ   NOT NULL DEFAULT now(),
    PRIMARY KEY (zone_code, server_id),
    FOREIGN KEY (zone_code) REFERENCES sovereignty_zone(code)
);
CREATE INDEX IF NOT EXISTS ix_server_registry_status ON server_registry(zone_code, status);

CREATE TABLE IF NOT EXISTS project_routing (
    project_id   CHAR(14)    NOT NULL PRIMARY KEY,
    home_zone    CHAR(2)     NOT NULL,
    current_zone CHAR(2)     NOT NULL,
    migrated_at  TIMESTAMPTZ,
    migrated_by  UUID,
    FOREIGN KEY (home_zone) REFERENCES sovereignty_zone(code),
    FOREIGN KEY (current_zone) REFERENCES sovereignty_zone(code)
);
CREATE INDEX IF NOT EXISTS ix_project_routing_current ON project_routing(current_zone, home_zone);

CREATE TABLE IF NOT EXISTS migration_request (
    request_id   UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id   CHAR(14)    NOT NULL,
    from_zone    CHAR(2)     NOT NULL,
    to_zone      CHAR(2)     NOT NULL,
    reason       TEXT        NOT NULL,
    requested_by UUID        NOT NULL,
    approved_by  UUID,
    status       VARCHAR(20) NOT NULL DEFAULT 'pending',
    scheduled_at TIMESTAMPTZ,
    started_at   TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    data_size_mb BIGINT,
    error_message TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (requested_by) REFERENCES dash_user(user_id),
    FOREIGN KEY (approved_by) REFERENCES dash_user(user_id),
    FOREIGN KEY (from_zone) REFERENCES sovereignty_zone(code),
    FOREIGN KEY (to_zone) REFERENCES sovereignty_zone(code)
);

CREATE TABLE IF NOT EXISTS migration_audit_log (
    log_id      UUID        NOT NULL PRIMARY KEY DEFAULT gen_random_uuid(),
    request_id  UUID        NOT NULL,
    step        VARCHAR(50) NOT NULL,
    status      VARCHAR(20) NOT NULL,
    details     JSONB,
    checksum    VARCHAR(128),
    duration_ms BIGINT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (request_id) REFERENCES migration_request(request_id)
);
CREATE INDEX IF NOT EXISTS ix_migration_audit_request ON migration_audit_log(request_id, created_at);

-- Insert root zones first (no fallback dependencies)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('us', 'USA Commercial',           'USA Commercial',                     NULL, FALSE, FALSE),
  ('sg', 'Singapore + ASEAN',        'Singapore + ASEAN',                  'us', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend only on us or sg
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('ug', 'USA GovCloud',             'USA GovCloud',                       'us', TRUE,  FALSE),
  ('uh', 'USA Healthcare',           'USA Healthcare',                     'us', TRUE,  FALSE),
  ('ca', 'Canada',                   'Canada',                             'us', FALSE, FALSE),
  ('mx', 'Mexico + Central Am + Caribbean', 'Mexico, Central America & Caribbean', 'us', FALSE, FALSE),
  ('sa', 'South America excl Brasil','South America (excl Brasil)',         'us', FALSE, FALSE),
  ('br', 'Brasil',                   'Brasil',                             'us', FALSE, FALSE),
  ('eu', 'Europe EU + EEA',          'Europe EU + EEA',                    'us', FALSE, FALSE),
  ('uk', 'United Kingdom',           'United Kingdom',                     'eu', FALSE, FALSE),
  ('jp', 'Japan',                    'Japan',                              'sg', FALSE, FALSE),
  ('in', 'India',                    'India',                              'sg', FALSE, FALSE),
  ('id', 'Indonesia',                'Indonesia',                          'sg', FALSE, FALSE),
  ('vn', 'Vietnam',                  'Vietnam',                            'sg', FALSE, FALSE),
  ('tw', 'Taiwan',                   'Taiwan',                             'sg', FALSE, FALSE),
  ('hk', 'Hong Kong',                'Hong Kong',                          'sg', FALSE, FALSE),
  ('ph', 'Philippines',              'Philippines',                        'sg', FALSE, FALSE),
  ('au', 'Australia + NZ',           'Australia + New Zealand',            'sg', FALSE, FALSE),
  ('af', 'Africa general',           'Africa',                             'eu', FALSE, FALSE),
  ('me', 'Middle East UAE/Saudi',    'Middle East (UAE/Saudi)',             'eu', FALSE, FALSE),
  ('il', 'Israel',                   'Israel',                             'eu', FALSE, FALSE),
  ('gl', 'Global / no residency',    'Global (No Data Residency)',         'us', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on ug (which depends on us)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('ud', 'USA DoD',                  'USA DoD',                            'ug', TRUE,  TRUE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on eu
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('es', 'Europe Sovereign',         'Europe Sovereign',                   'eu', TRUE,  FALSE),
  ('ru', 'Russia + CIS',             'Russia + CIS',                       'eu', TRUE,  FALSE),
  ('cn', 'China Commercial',         'China Commercial',                   'sg', TRUE,  FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on cn (which depends on sg)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('cg', 'China Government',         'China Government',                   'cn', TRUE,  TRUE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on jp (which depends on sg)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('kr', 'South Korea',              'South Korea',                        'jp', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on af (which depends on eu)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('ng', 'Nigeria',                  'Nigeria',                            'af', FALSE, FALSE),
  ('za', 'South Africa',             'South Africa',                       'af', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Insert zones that depend on me (which depends on eu)
INSERT INTO sovereignty_zone (code, name, display, fallback_zone, requires_approval, requires_mfa) VALUES
  ('qa', 'Qatar + Gulf',             'Qatar + Gulf',                       'me', FALSE, FALSE)
ON CONFLICT DO NOTHING;

-- Seed current US East server
INSERT INTO server_registry (server_id, zone_code, hostname, endpoint, internal_ip, db_url, status)
VALUES ('a20', 'us', 'alethedash-vm', 'https://alethedash.com', '20.42.8.158',
        'postgres://alethedash:alethedash@127.0.0.1/alethedash', 'active')
ON CONFLICT DO NOTHING;
