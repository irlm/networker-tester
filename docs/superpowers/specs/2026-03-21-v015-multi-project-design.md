# v0.15 Design Spec — Multi-Project Tenancy, Cloud Accounts, Share Links

## Overview

Introduce projects as the top-level organizational unit. All resources (deployments, tests, schedules, agents, runs) belong to a project. Users belong to multiple projects with per-project roles. Cloud provider credentials are scoped to projects. Test results can be shared publicly via expiring links.

This builds on v0.14 (SSO, email identity, three-tier RBAC, pending approval). v0.14 establishes identity and roles; v0.15 adds the organizational structure those roles operate within.

## Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Project model | Explicit `project` table, all resources FK to it | Clean boundaries, simple queries, natural audit trail |
| Role storage | `project_member.role` per-project, separate `dash_user.is_platform_admin` | Platform admin is orthogonal to project membership — avoids role confusion |
| Default project | Auto-create "Default" project on migration, move existing resources into it | Zero-downtime migration for existing single-project deployments |
| Cloud credential storage | AES-256-GCM encrypted at rest, key from env var | Credentials are high-value secrets; DB compromise alone should not expose them |
| Personal cloud accounts | `cloud_account.owner_id` nullable — NULL means project-shared, non-NULL means personal | Single table, no schema split; visibility logic in queries |
| Share link tokens | 32-byte random, URL-safe base64, stored as SHA-256 hash | Same pattern as password reset tokens (v0.14); raw token in URL, hash in DB |
| Project context in API | `/api/projects/:project_id/...` nested routes | Explicit, RESTful, easy to middleware-guard |
| Project switcher | Dropdown in sidebar header, persisted in localStorage | Low-friction switching, no page reload needed |
| WebSocket approval notifications | SSE (Server-Sent Events) on `/api/events/approval` | Simpler than WS for one-way server-push; avoids complexity of a second WS channel |
| Per-tester command approval | `command_approval` table, admin approves before execution | Prevents unauthorized destructive actions on VMs |

## Architecture

### Domain Model

```
Platform Admin
  └── manages all Projects + system settings

Project
  ├── Members (user + role per project)
  │     ├── Project Admin — manages this project
  │     ├── Operator — runs tests, deploys, uses approved accounts
  │     └── Viewer — read-only (scoped visibility)
  ├── Cloud Accounts (Azure / AWS / GCP credentials)
  ├── Deployments (endpoints)
  ├── Agents / Testers
  ├── Test Definitions
  ├── Jobs
  ├── Schedules
  ├── Runs / Results
  └── Share Links (public, expiring)
```

### Role Hierarchy

```
Platform Admin (is_platform_admin = true on dash_user)
  │  Can: manage all projects, system settings, promote other platform admins
  │  Scope: global
  │
  ├── Project Admin (project_member.role = 'admin')
  │     Can: manage members, cloud accounts, settings, visibility rules for this project
  │     Can: everything Operator can do
  │
  ├── Operator (project_member.role = 'operator')
  │     Can: run tests, create schedules, deploy VMs (using approved cloud accounts)
  │     Can: add personal cloud accounts (visible only to them)
  │     Can: everything Viewer can do
  │
  └── Viewer (project_member.role = 'viewer')
        Can: view test results (subject to visibility rules set by project admin)
        Cannot: create, modify, or delete anything
```

**Key distinction from v0.14**: The `dash_user.role` column from v0.14 becomes the user's default role suggestion (used when auto-creating project membership), but the authoritative role for any project operation is `project_member.role`. `dash_user.is_platform_admin` replaces the old `role = 'admin'` for system-wide admin checks.

### Project Context Flow

```
Browser request: POST /api/projects/abc-123/jobs
  │
  ├── require_auth middleware (v0.14) → extracts AuthUser { user_id, email }
  │
  ├── require_project middleware (new) → extracts ProjectContext { project_id, role }
  │     1. Read :project_id from path
  │     2. Query project_member for (user_id, project_id)
  │     3. If platform admin → role = 'admin' (implicit membership in all projects)
  │     4. If not a member → 403 "Not a member of this project"
  │     5. Insert ProjectContext into request extensions
  │
  ├── require_project_role("operator") → checks ProjectContext.role >= operator
  │
  └── Handler executes with project_id scoping all DB queries
```

## Database Changes (V009 Migration)

### New Tables

```sql
-- ============================================================
-- V009: Multi-project tenancy, cloud accounts, share links
-- ============================================================

-- Projects
CREATE TABLE IF NOT EXISTS project (
    project_id   UUID           NOT NULL PRIMARY KEY,
    name         VARCHAR(200)   NOT NULL,
    slug         VARCHAR(100)   NOT NULL UNIQUE,  -- URL-friendly identifier
    description  TEXT,
    created_by   UUID           REFERENCES dash_user(user_id),
    created_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    settings     JSONB          NOT NULL DEFAULT '{}'::jsonb
    -- settings contains: { test_visibility: "all" | "explicit", ... }
);

CREATE INDEX IF NOT EXISTS ix_project_slug ON project (slug);

-- Project membership (user ↔ project ↔ role)
CREATE TABLE IF NOT EXISTS project_member (
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    user_id      UUID           NOT NULL REFERENCES dash_user(user_id) ON DELETE CASCADE,
    role         VARCHAR(20)    NOT NULL DEFAULT 'viewer',
    -- role: 'admin' | 'operator' | 'viewer'
    joined_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    invited_by   UUID           REFERENCES dash_user(user_id),
    PRIMARY KEY (project_id, user_id)
);

CREATE INDEX IF NOT EXISTS ix_project_member_user ON project_member (user_id);

-- Cloud provider accounts (project-scoped or personal)
CREATE TABLE IF NOT EXISTS cloud_account (
    account_id       UUID           NOT NULL PRIMARY KEY,
    project_id       UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    owner_id         UUID           REFERENCES dash_user(user_id) ON DELETE CASCADE,
    -- NULL owner_id = project-shared (visible to all members)
    -- non-NULL owner_id = personal (visible only to owner)
    name             VARCHAR(200)   NOT NULL,
    provider         VARCHAR(20)    NOT NULL,  -- 'azure' | 'aws' | 'gcp'
    credentials_enc  BYTEA          NOT NULL,  -- AES-256-GCM encrypted JSON
    credentials_nonce BYTEA         NOT NULL,  -- 12-byte GCM nonce
    region_default   VARCHAR(100),             -- default region for this account
    status           VARCHAR(20)    NOT NULL DEFAULT 'active',
    -- status: 'active' | 'disabled' | 'error'
    last_validated   TIMESTAMPTZ,
    validation_error TEXT,
    created_at       TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_cloud_account_project ON cloud_account (project_id);
CREATE INDEX IF NOT EXISTS ix_cloud_account_owner ON cloud_account (owner_id) WHERE owner_id IS NOT NULL;

-- Share links (public, expiring URLs for test results)
CREATE TABLE IF NOT EXISTS share_link (
    link_id      UUID           NOT NULL PRIMARY KEY,
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    token_hash   VARCHAR(64)    NOT NULL UNIQUE,  -- SHA-256 of the raw token
    resource_type VARCHAR(20)   NOT NULL,  -- 'run' | 'job' | 'dashboard'
    resource_id  UUID,                     -- NULL for dashboard-level shares
    label        VARCHAR(200),             -- optional human-readable label
    expires_at   TIMESTAMPTZ    NOT NULL,
    created_by   UUID           NOT NULL REFERENCES dash_user(user_id),
    created_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    revoked      BOOLEAN        NOT NULL DEFAULT FALSE,
    access_count INT            NOT NULL DEFAULT 0,
    last_accessed TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS ix_share_link_token ON share_link (token_hash) WHERE revoked = FALSE;
CREATE INDEX IF NOT EXISTS ix_share_link_project ON share_link (project_id, resource_type);
CREATE INDEX IF NOT EXISTS ix_share_link_expires ON share_link (expires_at) WHERE revoked = FALSE;

-- Command approval (per-tester approval for destructive operations)
CREATE TABLE IF NOT EXISTS command_approval (
    approval_id  UUID           NOT NULL PRIMARY KEY,
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    agent_id     UUID           NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
    command_type VARCHAR(50)    NOT NULL,  -- 'vm_run_command' | 'vm_delete' | 'vm_stop'
    command_detail JSONB        NOT NULL,  -- { vm_name, resource_group, script_preview, ... }
    status       VARCHAR(20)    NOT NULL DEFAULT 'pending',
    -- status: 'pending' | 'approved' | 'denied' | 'expired'
    requested_by UUID           NOT NULL REFERENCES dash_user(user_id),
    decided_by   UUID           REFERENCES dash_user(user_id),
    requested_at TIMESTAMPTZ    NOT NULL DEFAULT now(),
    decided_at   TIMESTAMPTZ,
    expires_at   TIMESTAMPTZ    NOT NULL,  -- auto-expire after 1 hour
    reason       TEXT                       -- approval/denial reason
);

CREATE INDEX IF NOT EXISTS ix_command_approval_pending ON command_approval (project_id, status) WHERE status = 'pending';

-- Test visibility rules (which tests/schedules a viewer can see)
CREATE TABLE IF NOT EXISTS test_visibility_rule (
    rule_id       UUID           NOT NULL PRIMARY KEY,
    project_id    UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    user_id       UUID           REFERENCES dash_user(user_id) ON DELETE CASCADE,
    -- NULL user_id = applies to all viewers in this project
    -- non-NULL = applies to specific user
    resource_type VARCHAR(20)    NOT NULL,  -- 'test_definition' | 'schedule'
    resource_id   UUID           NOT NULL,
    created_by    UUID           NOT NULL REFERENCES dash_user(user_id),
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS ix_visibility_project ON test_visibility_rule (project_id, user_id, resource_type);
```

### Modifications to Existing Tables

```sql
-- Add project_id FK to all resource tables
ALTER TABLE agent ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE test_definition ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE job ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE deployment ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);

-- Add cloud_account_id to deployment (which account was used)
ALTER TABLE deployment ADD COLUMN IF NOT EXISTS cloud_account_id UUID REFERENCES cloud_account(account_id);

-- Add is_platform_admin to dash_user
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS is_platform_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- Migrate existing admin users to platform admin
-- (After v0.14 migration, role='admin' users become platform admins)
UPDATE dash_user SET is_platform_admin = TRUE WHERE role = 'admin';

-- Index for project-scoped queries
CREATE INDEX IF NOT EXISTS ix_agent_project ON agent (project_id);
CREATE INDEX IF NOT EXISTS ix_test_def_project ON test_definition (project_id);
CREATE INDEX IF NOT EXISTS ix_job_project ON job (project_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_schedule_project ON schedule (project_id) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS ix_deployment_project ON deployment (project_id, status, created_at DESC);
```

### Data Migration (Zero-Downtime)

```sql
-- Create "Default" project for existing single-project deployments
INSERT INTO project (project_id, name, slug, description)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Default',
    'default',
    'Auto-created during v0.15 migration. Contains all pre-existing resources.'
) ON CONFLICT DO NOTHING;

-- Move all existing resources into the Default project
UPDATE agent SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE test_definition SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE job SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE schedule SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE deployment SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;

-- Add all existing active users as admin of the Default project
INSERT INTO project_member (project_id, user_id, role)
SELECT '00000000-0000-0000-0000-000000000001', user_id, 'admin'
FROM dash_user
WHERE status = 'active'
ON CONFLICT DO NOTHING;

-- Make project_id NOT NULL after migration (separate statement, not in same transaction as data migration)
-- This is done in application code after verifying all rows have project_id set
-- ALTER TABLE agent ALTER COLUMN project_id SET NOT NULL;
-- ALTER TABLE test_definition ALTER COLUMN project_id SET NOT NULL;
-- ALTER TABLE job ALTER COLUMN project_id SET NOT NULL;
-- ALTER TABLE schedule ALTER COLUMN project_id SET NOT NULL;
-- ALTER TABLE deployment ALTER COLUMN project_id SET NOT NULL;
```

**Migration strategy**: The `project_id` columns are added as nullable first, data is migrated, then a follow-up V010 migration (or application-level check) sets them to NOT NULL. This allows rolling deployments where old code writes NULLs and the migration backfills them.

**Trade-off**: We use a well-known UUID for the Default project (`000...001`) instead of generating a random one. This makes the migration idempotent and allows hardcoding in tests. The downside is it is a magic constant — but it only matters during migration, not in steady-state operation.

### Entity-Relationship Summary

```
dash_user ──────────────┬── project_member ──── project
  │                     │                         │
  │ is_platform_admin   │ role per project        ├── cloud_account
  │                     │                         ├── agent
  └─────────────────────┘                         ├── test_definition
                                                  ├── job
                                                  ├── schedule
                                                  ├── deployment
                                                  ├── share_link
                                                  ├── command_approval
                                                  └── test_visibility_rule
```

## API Changes

### New Route Structure

All resource endpoints move under `/api/projects/:project_id/`. The old flat routes become redirects or are removed.

```
/api/projects                                    GET    — list user's projects
/api/projects                                    POST   — create project (platform admin)
/api/projects/:pid                               GET    — get project details
/api/projects/:pid                               PUT    — update project (project admin)
/api/projects/:pid                               DELETE — delete project (platform admin)

/api/projects/:pid/members                       GET    — list members (project admin)
/api/projects/:pid/members                       POST   — add member (project admin)
/api/projects/:pid/members/:uid                  PUT    — change member role (project admin)
/api/projects/:pid/members/:uid                  DELETE — remove member (project admin)

/api/projects/:pid/cloud-accounts                GET    — list cloud accounts (+ personal)
/api/projects/:pid/cloud-accounts                POST   — add cloud account
/api/projects/:pid/cloud-accounts/:aid           GET    — get account (no credentials exposed)
/api/projects/:pid/cloud-accounts/:aid           PUT    — update account
/api/projects/:pid/cloud-accounts/:aid           DELETE — delete account
/api/projects/:pid/cloud-accounts/:aid/validate  POST   — test credentials

/api/projects/:pid/agents                        GET    — list agents in project
/api/projects/:pid/jobs                          GET    — list jobs in project
/api/projects/:pid/jobs                          POST   — create job (operator+)
/api/projects/:pid/jobs/:jid                     GET    — get job detail
/api/projects/:pid/runs                          GET    — list runs in project
/api/projects/:pid/runs/:rid                     GET    — get run detail
/api/projects/:pid/schedules                     GET    — list schedules
/api/projects/:pid/schedules                     POST   — create schedule (operator+)
/api/projects/:pid/schedules/:sid                PUT    — update schedule (operator+)
/api/projects/:pid/schedules/:sid                DELETE — delete schedule (operator+)
/api/projects/:pid/deployments                   GET    — list deployments
/api/projects/:pid/deployments                   POST   — create deployment (operator+)
/api/projects/:pid/deployments/:did              GET    — get deployment detail

/api/projects/:pid/share-links                   GET    — list share links (admin)
/api/projects/:pid/share-links                   POST   — create share link (operator+)
/api/projects/:pid/share-links/:lid              PUT    — extend/revoke (admin)
/api/projects/:pid/share-links/:lid              DELETE — delete (admin)

/api/projects/:pid/visibility-rules              GET    — list rules (admin)
/api/projects/:pid/visibility-rules              POST   — add rule (admin)
/api/projects/:pid/visibility-rules/:rid         DELETE — remove rule (admin)

/api/projects/:pid/command-approvals             GET    — list pending approvals
/api/projects/:pid/command-approvals/:aid        POST   — approve/deny (admin)

-- Public share endpoint (no auth)
/share/:token                                    GET    — resolve share link, return resource

-- Project-unscoped endpoints (kept from v0.14)
/api/auth/*                                      — all auth endpoints (unchanged)
/api/users/*                                     — platform admin user management
/api/version                                     — dashboard version
/api/modes                                       — available test modes

-- SSE for real-time notifications
/api/events/approval                             GET    — SSE stream for pending approval notifications
/api/events/user-status                          GET    — SSE stream for user status changes (pending → active)
```

### Backward Compatibility

For the transition period (one release cycle), keep the old flat routes as thin redirects:

```rust
// Old: GET /api/jobs → redirect to /api/projects/{active_project}/jobs
// The redirect reads the user's "last active project" from their JWT or a cookie.
// This avoids breaking existing API consumers immediately.
```

After one release (v0.16), remove the redirect layer.

### Middleware Stack

```
Request
  │
  ├── CorsLayer (existing)
  ├── require_auth (v0.14 — JWT validation, status check)
  │     └── inserts AuthUser { user_id, email, is_platform_admin }
  ├── require_project (new — for /api/projects/:pid/* routes)
  │     └── inserts ProjectContext { project_id, project_slug, role }
  ├── require_project_role (new — parameterized: "viewer" | "operator" | "admin")
  │     └── checks ProjectContext.role >= required
  └── Handler
```

**`require_project` middleware logic:**

```rust
pub struct ProjectContext {
    pub project_id: Uuid,
    pub project_slug: String,
    pub role: ProjectRole,  // Admin | Operator | Viewer
}

async fn require_project(
    State(state): State<Arc<AppState>>,
    auth_user: Extension<AuthUser>,
    mut req: Request,
    next: Next,
) -> Response {
    // Extract project_id from path parameter
    let project_id = /* parse from :pid path segment */;

    // Platform admins have implicit admin access to all projects
    if auth_user.is_platform_admin {
        let project = /* fetch project by id */;
        req.extensions_mut().insert(ProjectContext {
            project_id,
            project_slug: project.slug,
            role: ProjectRole::Admin,
        });
        return next.run(req).await;
    }

    // Check membership
    let membership = /* query project_member for (user_id, project_id) */;
    match membership {
        Some(member) => {
            req.extensions_mut().insert(ProjectContext {
                project_id,
                project_slug: member.project_slug,
                role: member.role.parse(),
            });
            next.run(req).await
        }
        None => (StatusCode::FORBIDDEN, "Not a member of this project").into_response(),
    }
}
```

### Cloud Account API Details

**POST /api/projects/:pid/cloud-accounts**

```json
{
  "name": "Production Azure",
  "provider": "azure",
  "credentials": {
    "tenant_id": "...",
    "client_id": "...",
    "client_secret": "..."
  },
  "region_default": "eastus",
  "personal": false
}
```

- If `personal: true`, sets `owner_id` to the requesting user. Only operators and admins can add accounts.
- If `personal: false`, only project admins can add (shared accounts).
- Credentials are encrypted before storage (see Security section).
- Response never includes credentials — only metadata.

**GET /api/projects/:pid/cloud-accounts**

Returns all project-shared accounts plus the requesting user's personal accounts. Never returns other users' personal accounts.

```json
[
  {
    "account_id": "...",
    "name": "Production Azure",
    "provider": "azure",
    "region_default": "eastus",
    "personal": false,
    "status": "active",
    "last_validated": "2026-03-20T..."
  },
  {
    "account_id": "...",
    "name": "My AWS (personal)",
    "provider": "aws",
    "personal": true,
    "status": "active"
  }
]
```

### Share Link API Details

**POST /api/projects/:pid/share-links**

```json
{
  "resource_type": "run",
  "resource_id": "abc-123",
  "label": "Q1 latency report",
  "expires_in_days": 30
}
```

Response:

```json
{
  "link_id": "...",
  "url": "https://networker-dash.example.com/share/Kx9mR2...pQ7n",
  "expires_at": "2026-04-20T...",
  "label": "Q1 latency report"
}
```

**The raw token is returned only once** (at creation). It is stored as SHA-256 in the DB. If the user loses the URL, they must create a new share link.

**GET /share/:token (public, no auth)**

```
1. Hash the token: SHA-256(token)
2. Look up share_link where token_hash = hash AND revoked = FALSE AND expires_at > now()
3. If not found → 404
4. Increment access_count, update last_accessed
5. Fetch the resource (run, job, dashboard snapshot) scoped to share_link.project_id
6. Return the resource data (same shape as the authenticated endpoint, minus edit actions)
```

**Security constraints on share links:**
- Maximum expiration: 365 days (enforced server-side)
- Default expiration: 30 days
- Project admin can revoke at any time
- Rate limit: 100 requests/hour per token (prevent scraping)
- Share links do not grant access to modify anything — strictly read-only
- Share link tokens are 32 bytes (256 bits), URL-safe base64 encoded (43 chars)

### Command Approval Flow

```
Operator clicks "Update endpoint" on a deployed VM
  │
  ├── Dashboard creates command_approval record (status = 'pending', expires_at = now + 1h)
  ├── SSE notification pushed to project admins: "Approval requested: update endpoint on vm-eastus-01"
  │
  ├── Admin sees notification in sidebar (bell icon with badge count)
  ├── Admin reviews: VM name, command preview, who requested, when
  ├── Admin clicks Approve (or Deny with reason)
  │
  ├── Dashboard updates command_approval record
  ├── If approved: executes `az vm run-command invoke` with the approved script
  ├── SSE notification to requester: "Your command was approved/denied"
  │
  └── Expired approvals are cleaned up by the scheduler (hourly)
```

### SSE for Real-Time Notifications

**Why SSE instead of extending the existing WebSocket:**

The existing `/ws/dashboard` WebSocket carries high-frequency data (live test results, agent heartbeats). Mixing low-frequency notification events into it would complicate the message protocol and require all WS consumers to filter messages they don't care about. SSE is simpler for one-way server-push of infrequent events.

**Trade-off:** Two connections per browser tab (WS + SSE) instead of one. Acceptable given the low overhead of SSE and the cleaner separation of concerns.

**`GET /api/events/approval` (SSE, JWT required)**

```
event: approval_requested
data: {"approval_id":"...","agent_name":"vm-eastus-01","command_type":"vm_run_command","requested_by":"alice@co.com"}

event: approval_decided
data: {"approval_id":"...","status":"approved","decided_by":"bob@co.com"}
```

**`GET /api/events/user-status` (SSE, JWT required)**

Replaces the 10-second polling on the `/pending` page (v0.14).

```
event: status_changed
data: {"status":"active","role":"operator","project_id":"..."}
```

## Frontend Changes

### New Pages

| Page | Route | Access |
|------|-------|--------|
| Projects List | `/projects` | Any authenticated user |
| Project Settings | `/projects/:pid/settings` | Project admin |
| Project Members | `/projects/:pid/members` | Project admin |
| Cloud Accounts | `/projects/:pid/cloud-accounts` | Project admin (manage), Operator (view/add personal) |
| Share Links | `/projects/:pid/share-links` | Project admin |
| Command Approvals | `/projects/:pid/approvals` | Project admin |
| Public Share View | `/share/:token` | Public (no auth) |

### Modified Pages

All existing pages gain project context from the URL:

| Current Route | New Route |
|---------------|-----------|
| `/` | `/projects/:pid` (dashboard) |
| `/tests` | `/projects/:pid/tests` |
| `/tests/:jobId` | `/projects/:pid/tests/:jobId` |
| `/runs` | `/projects/:pid/runs` |
| `/runs/:runId` | `/projects/:pid/runs/:runId` |
| `/deploy` | `/projects/:pid/deploy` |
| `/deploy/:did` | `/projects/:pid/deploy/:did` |
| `/schedules` | `/projects/:pid/schedules` |
| `/settings` | `/projects/:pid/settings` |

The root route `/` redirects to `/projects/:lastActiveProjectId` (from localStorage) or `/projects` if no project is selected.

### Project Switcher (Sidebar)

```
┌──────────────────────────────┐
│  ▾ Production Network        │  ← dropdown trigger
│    ○ connected               │
├──────────────────────────────┤
│  Dashboard                   │
│  Deploy                      │
│  Tests                       │
│  Schedules                   │
│  Runs                        │
│  ──────────────              │
│  Cloud Accounts              │  ← new (operator+)
│  Share Links                 │  ← new (operator+)
│  ──────────────              │
│  Members                     │  ← new (project admin)
│  Project Settings            │  ← new (project admin)
│  ──────────────              │
│  Platform Settings           │  ← new (platform admin only)
│  Users                       │  ← moved from v0.14 (platform admin)
│  ──────────────              │
│  alice@company.com           │
│  Sign out                    │
└──────────────────────────────┘
```

**Dropdown behavior:**
- Shows list of user's projects with role badge
- "Create Project" link at bottom (platform admin only)
- Selecting a project navigates to `/projects/:pid` and stores in localStorage
- Badge count for pending approvals (project admin)

### Auth Store Changes

```typescript
interface AuthState {
  token: string | null;
  email: string | null;
  role: string | null;          // kept for backward compat, but project role takes precedence
  status: string | null;
  isPlatformAdmin: boolean;
  isAuthenticated: boolean;
  mustChangePassword: boolean;

  // Project context
  activeProjectId: string | null;
  activeProjectSlug: string | null;
  activeProjectRole: string | null;  // 'admin' | 'operator' | 'viewer'
  projects: ProjectSummary[];        // cached list of user's projects

  login: (token, email, role, status, isPlatformAdmin, mustChangePassword?) => void;
  setActiveProject: (projectId, slug, role) => void;
  setProjects: (projects: ProjectSummary[]) => void;
  logout: () => void;
}

interface ProjectSummary {
  project_id: string;
  name: string;
  slug: string;
  role: string;
}
```

### JWT Claims (Updated)

```rust
struct Claims {
    sub: Uuid,              // user_id
    email: String,
    role: String,           // legacy — user's default role
    is_platform_admin: bool,
    exp: i64,
    iat: i64,
}
```

Project role is NOT in the JWT. It is fetched per-request from the DB via the `require_project` middleware. This avoids token invalidation when a user's project role changes.

**Trade-off:** Extra DB query per request for project-scoped endpoints. Mitigate with a short-lived in-memory cache (5-minute TTL, keyed by `(user_id, project_id)`). The existing `require_auth` already does a DB query for `must_change_password`, so the overhead is bounded.

### Deploy Wizard Changes

The deploy wizard gains a cloud account selector:

```
Step 1: Select Provider
  [Azure] [AWS] [GCP] [LAN]

Step 2: Select Account (new)
  ┌─────────────────────────────────┐
  │  Production Azure (shared)   ✓  │
  │  Staging Azure (shared)         │
  │  My Personal AWS (personal)     │
  │  ──────────────────             │
  │  + Add new account              │
  └─────────────────────────────────┘

Step 3: Configure VM (existing flow)
  ...
```

When the operator selects an account, the deployment uses that account's credentials (decrypted server-side) to provision the VM. The credentials are never sent to the browser.

### Viewer Visibility

When `project.settings.test_visibility = "explicit"`:
- Viewers see only tests/schedules listed in `test_visibility_rule` for their user (or rules with `user_id = NULL` which apply to all viewers)
- Operators and admins see everything
- The Jobs and Schedules API endpoints filter based on this

When `project.settings.test_visibility = "all"` (default):
- All project members see all tests and schedules

UI for managing visibility (project admin):
- On the Tests page, each test definition gets a "Visibility" dropdown: "All viewers" / "Specific viewers"
- When "Specific viewers" is selected, a multi-select of viewer members appears

### Public Share View (`/share/:token`)

A standalone page with no sidebar, no auth required:

```
┌────────────────────────────────────────────────┐
│  Networker Dashboard — Shared Report           │
│  ─────────────────────────────────────────────  │
│                                                │
│  [Full RunDetailPage content rendered here]     │
│  (charts, statistics, protocol breakdown, etc.) │
│                                                │
│  ─────────────────────────────────────────────  │
│  Shared by alice@company.com · Expires Apr 20   │
│  Powered by Networker                          │
└────────────────────────────────────────────────┘
```

The share view reuses the existing `RunDetailPage` component but wraps it in a read-only context (no edit buttons, no sidebar, no navigation to other pages).

### Notification Bell (Sidebar)

Project admins see a bell icon in the sidebar with a count of pending command approvals. Clicking opens a drawer listing pending approvals with approve/deny buttons.

## Security Considerations

### Cloud Credential Encryption

**Encryption at rest**: Credentials are encrypted with AES-256-GCM before storing in `cloud_account.credentials_enc`.

```rust
// Encryption key from environment variable
// DASHBOARD_CREDENTIAL_KEY = 32-byte hex string (64 hex chars)
// Example: DASHBOARD_CREDENTIAL_KEY=a1b2c3d4...

fn encrypt_credentials(plaintext: &[u8], key: &[u8; 32]) -> (Vec<u8>, [u8; 12]) {
    use aes_gcm::{Aes256Gcm, Key, Nonce, aead::Aead, KeyInit};
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);
    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext).expect("encryption");
    (ciphertext, nonce_bytes)
}

fn decrypt_credentials(ciphertext: &[u8], nonce: &[u8; 12], key: &[u8; 32]) -> Vec<u8> {
    use aes_gcm::{Aes256Gcm, Key, Nonce, aead::Aead, KeyInit};
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce);
    cipher.decrypt(nonce, ciphertext).expect("decryption")
}
```

**Key management:**
- `DASHBOARD_CREDENTIAL_KEY` is a required env var when cloud accounts are used. If absent, cloud account creation returns a 503 with a clear error message.
- Key rotation: add a `key_version` column to `cloud_account`. When the key rotates, re-encrypt all credentials in a migration. Old key is kept in `DASHBOARD_CREDENTIAL_KEY_OLD` until all rows are migrated.
- **Risk**: If the encryption key is compromised, all cloud credentials are exposed. Mitigate: store the key in Azure Key Vault / AWS Secrets Manager in production, inject via env var at deploy time.

**Trade-off**: Application-level encryption (vs. PostgreSQL `pgcrypto` column encryption). Application-level is chosen because: (a) the key stays out of the DB, (b) it works the same across any PostgreSQL provider, (c) decryption happens only when needed (deploying a VM), not on every query.

### Credential Format per Provider

```json
// Azure Service Principal
{
  "tenant_id": "...",
  "client_id": "...",
  "client_secret": "..."
}

// AWS Access Keys
{
  "access_key_id": "AKIA...",
  "secret_access_key": "..."
}

// GCP Service Account Key
{
  "type": "service_account",
  "project_id": "...",
  "private_key_id": "...",
  "private_key": "-----BEGIN PRIVATE KEY-----\n...",
  "client_email": "...",
  ...
}
```

### Share Link Token Security

- 32-byte random tokens (256 bits of entropy) — brute-force infeasible
- Stored as SHA-256 hash — DB leak does not expose valid tokens
- Expiration enforced server-side on every access
- Revocation is instant (set `revoked = TRUE`)
- Rate-limited to 100 requests/hour per token hash (prevents scraping)
- No escalation: share links grant read-only access to a single resource, never to the project or other resources

### Project Isolation

- Every query on a project-scoped table includes `WHERE project_id = $1` — no cross-project data leakage
- The `require_project` middleware ensures users can only access projects they are members of
- Platform admins bypass membership checks but are still subject to project_id scoping in queries
- Foreign key cascading (`ON DELETE CASCADE`) ensures deleting a project removes all its data
- Cloud account credentials are project-scoped — a user with access to Project A cannot use Project B's Azure credentials even if they are a member of both projects (credentials are selected per-deployment)

### Agent Authentication with Projects

Currently agents authenticate with an API key. In v0.15, agents are assigned to a project:

- Agent registration includes `project_id` in the request
- The API key is scoped to a project — an agent can only see jobs and submit results for its project
- Agent hub (`ws/agent_hub.rs`) filters messages by project_id
- Browser hub (`ws/browser_hub.rs`) filters events by the user's active project

## Configuration Changes

### New Environment Variables

```bash
# Cloud credential encryption (required for cloud account feature)
DASHBOARD_CREDENTIAL_KEY=<64-hex-chars>

# Optional: old key for rotation
DASHBOARD_CREDENTIAL_KEY_OLD=<64-hex-chars>

# Share link public URL prefix (defaults to DASHBOARD_PUBLIC_URL)
DASHBOARD_SHARE_URL=https://networker-dash.example.com

# Maximum share link expiration in days (default: 365)
DASHBOARD_SHARE_MAX_DAYS=365
```

### AppState Changes

```rust
pub struct AppState {
    pub db: deadpool_postgres::Pool,
    pub database_url: String,
    pub jwt_secret: String,
    pub dashboard_port: u16,
    pub events_tx: broadcast::Sender<DashboardEvent>,
    pub agents: AgentHub,
    pub tester_processes: RwLock<HashMap<Uuid, u32>>,

    // v0.15 additions
    pub credential_key: Option<[u8; 32]>,        // AES-256 key for cloud credentials
    pub credential_key_old: Option<[u8; 32]>,    // Previous key for rotation
    pub share_base_url: String,                   // Base URL for share links
    pub share_max_days: u32,                      // Maximum share link duration

    // SSE channels
    pub approval_tx: broadcast::Sender<ApprovalEvent>,
    pub user_status_tx: broadcast::Sender<UserStatusEvent>,
}
```

## Rust Module Changes

### New Modules

```
crates/networker-dashboard/src/
  api/
    projects.rs          — CRUD for projects
    project_members.rs   — project membership management
    cloud_accounts.rs    — cloud account CRUD + validation
    share_links.rs       — share link CRUD + public resolve
    command_approvals.rs — approval request/decide
    visibility.rs        — test visibility rules
    events.rs            — SSE endpoints for approval + user status notifications
  db/
    projects.rs          — project queries
    project_members.rs   — membership queries
    cloud_accounts.rs    — cloud account queries (encrypt/decrypt)
    share_links.rs       — share link queries
    command_approvals.rs — approval queries
    visibility.rs        — visibility rule queries
  crypto.rs              — AES-256-GCM encrypt/decrypt helpers
```

### Modified Modules

```
api/mod.rs         — add project-scoped router, backward-compat redirects
api/agents.rs      — add project_id filtering
api/jobs.rs        — add project_id filtering
api/runs.rs        — add project_id filtering
api/schedules.rs   — add project_id filtering
api/deployments.rs — add project_id filtering, cloud_account_id selection
api/dashboard.rs   — add project_id filtering for summary stats
api/cloud.rs       — delegate to cloud_accounts.rs for credential-based operations
api/inventory.rs   — scope inventory by project's cloud accounts

auth/mod.rs        — add is_platform_admin to Claims + AuthUser
                   — add require_project and require_project_role middleware

db/migrations.rs   — add V009_MULTI_PROJECT migration
db/agents.rs       — add project_id to queries
db/jobs.rs         — add project_id to queries
db/runs.rs         — add project_id to queries (runs link to jobs which link to projects)
db/schedules.rs    — add project_id to queries
db/deployments.rs  — add project_id + cloud_account_id to queries
db/users.rs        — add is_platform_admin handling

ws/agent_hub.rs    — filter by project_id
ws/browser_hub.rs  — filter events by project_id

deploy/runner.rs   — read credentials from cloud_account instead of ambient env/CLI login
                   — check command_approval before executing destructive commands

main.rs            — add credential_key, SSE channels to AppState
config.rs          — add new env vars
```

## Frontend Module Changes

### New Files

```
dashboard/src/
  pages/
    ProjectsPage.tsx          — list projects, create new (platform admin)
    ProjectSettingsPage.tsx   — project name, description, visibility settings
    ProjectMembersPage.tsx    — manage members and roles
    CloudAccountsPage.tsx     — manage cloud accounts
    ShareLinksPage.tsx        — manage share links
    ShareViewPage.tsx         — public share view (no auth)
    CommandApprovalsPage.tsx  — approve/deny pending commands
  components/
    ProjectSwitcher.tsx       — sidebar dropdown
    CloudAccountSelector.tsx  — deploy wizard step
    ShareDialog.tsx           — create share link dialog
    NotificationBell.tsx      — sidebar bell icon with approval count
    ApprovalDrawer.tsx        — slide-out panel for approvals
  hooks/
    useProject.ts             — read project context from URL + store
    useSSE.ts                 — SSE connection hook (for approval + status events)
  stores/
    projectStore.ts           — project list, active project, project role
```

### Modified Files

```
App.tsx            — add project-scoped routes, share route, projects list route
stores/authStore.ts — add isPlatformAdmin, project context fields
components/layout/Sidebar.tsx — add project switcher, new nav entries, notification bell
components/DeployWizard.tsx   — add cloud account selection step
hooks/useWebSocket.ts         — add project_id to WS connection URL
```

### Updated App.tsx Route Structure

```tsx
function App() {
  return (
    <BrowserRouter>
      <Routes>
        {/* Public routes */}
        <Route path="/login" element={<LoginPage />} />
        <Route path="/forgot-password" element={<ForgotPasswordPage />} />
        <Route path="/reset-password" element={<ResetPasswordPage />} />
        <Route path="/auth/sso-complete" element={<SSOCompletePage />} />
        <Route path="/pending" element={<PendingPage />} />
        <Route path="/share/:token" element={<ShareViewPage />} />

        {/* Authenticated routes */}
        <Route path="/*" element={
          isAuthenticated ? <AuthenticatedApp /> : <Navigate to="/login" />
        } />
      </Routes>
    </BrowserRouter>
  );
}

function AuthenticatedApp() {
  return (
    <div className="flex min-h-screen bg-[var(--bg-base)]">
      <Sidebar />
      <main className="flex-1 overflow-auto pt-12 md:pt-0">
        <Routes>
          {/* Project list */}
          <Route path="/projects" element={<ProjectsPage />} />

          {/* Project-scoped routes */}
          <Route path="/projects/:projectId" element={<DashboardPage />} />
          <Route path="/projects/:projectId/tests" element={<JobsPage />} />
          <Route path="/projects/:projectId/tests/:jobId" element={<JobDetailPage />} />
          <Route path="/projects/:projectId/runs" element={<RunsPage />} />
          <Route path="/projects/:projectId/runs/:runId" element={<RunDetailPage />} />
          <Route path="/projects/:projectId/deploy" element={<DeployPage />} />
          <Route path="/projects/:projectId/deploy/:did" element={<DeployDetailPage />} />
          <Route path="/projects/:projectId/schedules" element={<SchedulesPage />} />
          <Route path="/projects/:projectId/settings" element={<ProjectSettingsPage />} />
          <Route path="/projects/:projectId/members" element={<ProjectMembersPage />} />
          <Route path="/projects/:projectId/cloud-accounts" element={<CloudAccountsPage />} />
          <Route path="/projects/:projectId/share-links" element={<ShareLinksPage />} />
          <Route path="/projects/:projectId/approvals" element={<CommandApprovalsPage />} />

          {/* Platform admin routes */}
          <Route path="/users" element={<UsersPage />} />
          <Route path="/change-password" element={<ChangePasswordPage />} />

          {/* Redirect root to last active project */}
          <Route path="/" element={<ProjectRedirect />} />

          {/* Backward compat redirects */}
          <Route path="/tests" element={<LegacyRedirect to="tests" />} />
          <Route path="/runs" element={<LegacyRedirect to="runs" />} />
          <Route path="/deploy" element={<LegacyRedirect to="deploy" />} />
          <Route path="/schedules" element={<LegacyRedirect to="schedules" />} />
        </Routes>
      </main>
    </div>
  );
}
```

## Migration Strategy

### Phase 1: Schema + Default Project (PR 1-2)

1. V009 migration adds new tables and nullable `project_id` columns
2. Creates "Default" project with well-known UUID
3. Migrates all existing resources into Default project
4. All existing users become admins of Default project
5. Existing admin users get `is_platform_admin = TRUE`
6. Application code treats NULL `project_id` as "Default project" during transition

**Behavioral change for users:** None yet. The Default project is auto-selected. UI looks the same.

### Phase 2: Project-Scoped API (PR 3-4)

1. New project-scoped endpoints go live alongside old flat endpoints
2. Old endpoints redirect to `/api/projects/{default_project_id}/...`
3. `require_project` middleware added to new routes
4. Frontend updated to use project-scoped URLs

**Behavioral change:** URLs change from `/tests` to `/projects/default/tests`. Sidebar shows project name.

### Phase 3: Multi-Project Features (PR 5-7)

1. Project creation UI (platform admin)
2. Member management UI
3. Cloud account management
4. Share links
5. Command approvals + SSE notifications

**Behavioral change:** Users can now create and switch between projects.

### Phase 4: Hardening (PR 8)

1. V010 migration: set `project_id` columns to NOT NULL
2. Remove old flat endpoint redirects
3. Remove backward-compat "NULL project_id = default" logic

### Rollback Plan

Each phase is independently deployable and rollback-safe:
- Phase 1: Drop V009 migration tables, remove nullable columns (no data loss since existing data is untouched)
- Phase 2: Revert to flat endpoints (data is still in Default project, accessible via old routes)
- Phase 3: Features are additive — disabling is safe
- Phase 4: Only proceed when confident phases 1-3 are stable

## Implementation Order (PRs)

| PR | Title | Depends On | Scope |
|----|-------|------------|-------|
| 1 | **DB: V009 multi-project schema + data migration** | v0.14 merged | New tables, nullable project_id columns, Default project creation, data migration |
| 2 | **Backend: project CRUD + membership API** | PR 1 | `api/projects.rs`, `api/project_members.rs`, `db/projects.rs`, `db/project_members.rs` |
| 3 | **Backend: project middleware + scoped resource APIs** | PR 2 | `require_project` middleware, refactor all resource APIs to accept project_id, backward-compat redirects |
| 4 | **Frontend: project switcher + scoped routes** | PR 3 | `ProjectSwitcher.tsx`, `projectStore.ts`, update App.tsx routes, update all page components to use `useProject()` |
| 5 | **Backend: cloud account encryption + API** | PR 3 | `crypto.rs`, `api/cloud_accounts.rs`, `db/cloud_accounts.rs`, `DASHBOARD_CREDENTIAL_KEY` env var |
| 6 | **Frontend: cloud accounts page + deploy wizard integration** | PR 4, PR 5 | `CloudAccountsPage.tsx`, `CloudAccountSelector.tsx`, update `DeployWizard.tsx` |
| 7 | **Backend + frontend: share links** | PR 3, PR 4 | `api/share_links.rs`, `db/share_links.rs`, `ShareLinksPage.tsx`, `ShareViewPage.tsx`, `ShareDialog.tsx`, `/share/:token` public route |
| 8 | **Backend + frontend: command approval + SSE notifications** | PR 3, PR 4 | `api/command_approvals.rs`, `api/events.rs`, `db/command_approvals.rs`, `CommandApprovalsPage.tsx`, `NotificationBell.tsx`, `useSSE.ts` |
| 9 | **Backend + frontend: test visibility rules** | PR 3, PR 4 | `api/visibility.rs`, `db/visibility.rs`, viewer filtering in jobs/schedules APIs, visibility UI on Tests page |
| 10 | **Hardening: NOT NULL migration + remove compat redirects** | PR 1-9 stable | V010 migration, remove backward-compat code |

**Estimated effort:** PRs 1-4 are the critical path (project infrastructure). PRs 5-9 can be parallelized across developers after PR 3-4 land. PR 10 ships after one release cycle of soak time.

## Version Bump

v0.14.x --> v0.15.0

All 3 sync locations: Cargo.toml workspace version, CHANGELOG.md, install.sh/install.ps1 INSTALLER_VERSION.

## Out of Scope (v0.16+)

- **Project templates** — pre-configured test definitions and schedules for common scenarios
- **Cross-project dashboards** — aggregate view across multiple projects
- **Audit log** — full event log of who did what in each project (currently covered partially by deployment logs)
- **API keys for projects** — machine-to-machine access scoped to a project (currently only agent API keys exist)
- **SAML / SCIM** — enterprise SSO protocol + automated user provisioning
- **Billing / usage quotas** — per-project resource limits
- **Project archival** — soft-delete projects instead of hard-delete
- **Credential vault integration** — direct Azure Key Vault / AWS Secrets Manager integration instead of env-var key
