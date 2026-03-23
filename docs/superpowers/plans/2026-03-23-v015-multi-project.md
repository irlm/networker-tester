# v0.15 Multi-Project Tenancy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce projects as the top-level organizational unit. All dashboard-visible resources (deployments, tests, schedules, agents, runs, URL tests) become project-scoped. Users belong to multiple projects with per-project roles. Cloud provider credentials are encrypted and stored per-project. Test results can be shared via expiring public links. Destructive VM commands require admin approval with SSE real-time notifications.

**Architecture:** Ten PRs in dependency order. PRs 1-4 are the critical path (project infrastructure). PRs 5-9 can be parallelized after PR 3-4 land. PR 10 ships after one release cycle of soak time. Each PR is independently deployable and rollback-safe.

**Tech Stack:** Rust (axum 0.7, tokio-postgres, aes-gcm, sha2), React 19 (TypeScript, Zustand 5, Tailwind 4, React Router), PostgreSQL 16, SSE (Server-Sent Events)

**Spec:** `docs/superpowers/specs/2026-03-21-v015-multi-project-design.md`

**Migration numbering note:** V009 is already used by cloud_connections (v0.14). This plan uses **V010** for multi-project schema and **V011** for the NOT NULL hardening.

**Default project UUID:** `00000000-0000-0000-0000-000000000001` (well-known, idempotent, hardcoded in tests)

---

## File Map

### Files to Create

| File | PR | Purpose |
|------|-----|---------|
| `crates/networker-dashboard/src/api/projects.rs` | 2 | Project CRUD endpoints |
| `crates/networker-dashboard/src/api/project_members.rs` | 2 | Project membership management endpoints |
| `crates/networker-dashboard/src/api/cloud_accounts.rs` | 5 | Cloud account CRUD + validation endpoints |
| `crates/networker-dashboard/src/api/share_links.rs` | 7 | Share link CRUD + public resolve endpoint |
| `crates/networker-dashboard/src/api/command_approvals.rs` | 8 | Approval request/decide endpoints |
| `crates/networker-dashboard/src/api/visibility.rs` | 9 | Test visibility rule endpoints |
| `crates/networker-dashboard/src/api/events.rs` | 8 | SSE endpoints for approval + user status notifications |
| `crates/networker-dashboard/src/db/projects.rs` | 2 | Project + membership DB queries |
| `crates/networker-dashboard/src/db/cloud_accounts.rs` | 5 | Cloud account queries (encrypt/decrypt) |
| `crates/networker-dashboard/src/db/share_links.rs` | 7 | Share link queries |
| `crates/networker-dashboard/src/db/command_approvals.rs` | 8 | Approval queries |
| `crates/networker-dashboard/src/db/visibility.rs` | 9 | Visibility rule queries |
| `crates/networker-dashboard/src/crypto.rs` | 5 | AES-256-GCM encrypt/decrypt helpers |
| `dashboard/src/pages/ProjectsPage.tsx` | 4 | List projects, create new (platform admin) |
| `dashboard/src/pages/ProjectSettingsPage.tsx` | 4 | Project name, description, visibility settings |
| `dashboard/src/pages/ProjectMembersPage.tsx` | 4 | Manage members and roles |
| `dashboard/src/pages/CloudAccountsPage.tsx` | 6 | Manage cloud accounts |
| `dashboard/src/pages/ShareLinksPage.tsx` | 7 | Manage share links |
| `dashboard/src/pages/ShareViewPage.tsx` | 7 | Public share view (no auth) |
| `dashboard/src/pages/CommandApprovalsPage.tsx` | 8 | Approve/deny pending commands |
| `dashboard/src/components/ProjectSwitcher.tsx` | 4 | Sidebar dropdown for project selection |
| `dashboard/src/components/CloudAccountSelector.tsx` | 6 | Deploy wizard cloud account step |
| `dashboard/src/components/ShareDialog.tsx` | 7 | Create share link dialog |
| `dashboard/src/components/NotificationBell.tsx` | 8 | Sidebar bell icon with approval count |
| `dashboard/src/components/ApprovalDrawer.tsx` | 8 | Slide-out panel for approvals |
| `dashboard/src/hooks/useProject.ts` | 4 | Read project context from URL + store |
| `dashboard/src/hooks/useSSE.ts` | 8 | SSE connection hook |
| `dashboard/src/stores/projectStore.ts` | 4 | Project list, active project, project role |

### Files to Modify (by PR)

**PR 1:** `db/migrations.rs`

**PR 2:** `api/mod.rs`, `api/projects.rs` (create), `api/project_members.rs` (create), `db/mod.rs`, `db/projects.rs` (create), `auth/mod.rs`

**PR 3:** `auth/mod.rs`, `api/mod.rs`, `api/agents.rs`, `api/jobs.rs`, `api/runs.rs`, `api/url_tests.rs`, `api/schedules.rs`, `api/deployments.rs`, `api/dashboard.rs`, `api/cloud.rs`, `api/inventory.rs`, `api/cloud_connections.rs`, `db/agents.rs`, `db/jobs.rs`, `db/runs.rs`, `db/url_tests.rs`, `db/schedules.rs`, `db/deployments.rs`, `db/cloud_connections.rs`, `ws/agent_hub.rs`, `ws/browser_hub.rs`

**PR 4:** `App.tsx`, `stores/authStore.ts`, `components/layout/Sidebar.tsx`, `hooks/useWebSocket.ts`, `api/client.ts`, `api/types.ts`, all existing page components (add `useProject()` hook)

**PR 5:** `Cargo.toml` (add `aes-gcm`), `config.rs`, `main.rs`, `api/mod.rs`, `db/mod.rs`

**PR 6:** `components/DeployWizard.tsx`, `api/client.ts`

**PR 7:** `api/mod.rs`, `db/mod.rs`, `App.tsx`, `api/client.ts`

**PR 8:** `api/mod.rs`, `db/mod.rs`, `main.rs`, `config.rs`, `deploy/runner.rs`, `components/layout/Sidebar.tsx`

**PR 9:** `api/jobs.rs`, `api/schedules.rs`, `db/jobs.rs`, `db/schedules.rs`, `db/mod.rs`, `api/mod.rs`

**PR 10:** `db/migrations.rs`, `api/mod.rs`, `auth/mod.rs`, `db/*.rs` (remove NULL coalescing), `Cargo.toml`, `CHANGELOG.md`, `install.sh`, `install.ps1`

---

## Dependency Chain

```
PR 1 (V010 migration)
  |
  v
PR 2 (project CRUD + membership API)
  |
  v
PR 3 (project middleware + scoped resource APIs)
  |
  +------+------+------+------+
  |      |      |      |      |
  v      v      v      v      v
PR 4   PR 5   PR 7   PR 8   PR 9
(FE)  (cloud) (share) (cmd)  (visibility)
  |      |
  v      v
PR 6   (PR 6 also depends on PR 4)
(cloud FE)
  |
  +--- all PRs 1-9 stable ---+
                              |
                              v
                           PR 10 (hardening + version bump)
```

---

## Task 1: V010 Multi-Project Schema + Data Migration (PR 1)

**Branch:** `feat/v015-schema-multi-project`

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`

This PR adds the database schema only. No application code changes. This is safe to deploy on top of v0.14.x -- the new tables and nullable columns do not affect existing functionality.

- [ ] **Step 1: Write V010_MULTI_PROJECT migration constant**

Add a new constant `V010_MULTI_PROJECT` in `db/migrations.rs` after the existing `V009_CLOUD_CONNECTIONS` constant. The SQL must execute in this exact order:

```sql
-- ============================================================
-- V010: Multi-project tenancy, cloud accounts, share links
-- ============================================================

-- 1. Projects table
CREATE TABLE IF NOT EXISTS project (
    project_id   UUID           NOT NULL PRIMARY KEY,
    name         VARCHAR(200)   NOT NULL,
    slug         VARCHAR(100)   NOT NULL UNIQUE,
    description  TEXT,
    created_by   UUID           REFERENCES dash_user(user_id),
    created_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ    NOT NULL DEFAULT now(),
    settings     JSONB          NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX IF NOT EXISTS ix_project_slug ON project (slug);

-- 2. Project membership
CREATE TABLE IF NOT EXISTS project_member (
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    user_id      UUID           NOT NULL REFERENCES dash_user(user_id) ON DELETE CASCADE,
    role         VARCHAR(20)    NOT NULL DEFAULT 'viewer',
    joined_at    TIMESTAMPTZ    NOT NULL DEFAULT now(),
    invited_by   UUID           REFERENCES dash_user(user_id),
    PRIMARY KEY (project_id, user_id)
);
CREATE INDEX IF NOT EXISTS ix_project_member_user ON project_member (user_id);

-- 3. Cloud accounts (project-scoped, encrypted credentials)
CREATE TABLE IF NOT EXISTS cloud_account (
    account_id       UUID           NOT NULL PRIMARY KEY,
    project_id       UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    owner_id         UUID           REFERENCES dash_user(user_id) ON DELETE CASCADE,
    name             VARCHAR(200)   NOT NULL,
    provider         VARCHAR(20)    NOT NULL,
    credentials_enc  BYTEA          NOT NULL,
    credentials_nonce BYTEA         NOT NULL,
    region_default   VARCHAR(100),
    status           VARCHAR(20)    NOT NULL DEFAULT 'active',
    last_validated   TIMESTAMPTZ,
    validation_error TEXT,
    created_at       TIMESTAMPTZ    NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_cloud_account_project ON cloud_account (project_id);
CREATE INDEX IF NOT EXISTS ix_cloud_account_owner ON cloud_account (owner_id) WHERE owner_id IS NOT NULL;

-- 4. Share links
CREATE TABLE IF NOT EXISTS share_link (
    link_id      UUID           NOT NULL PRIMARY KEY,
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    token_hash   VARCHAR(64)    NOT NULL UNIQUE,
    resource_type VARCHAR(20)   NOT NULL,
    resource_id  UUID,
    label        VARCHAR(200),
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

-- 5. Command approval
CREATE TABLE IF NOT EXISTS command_approval (
    approval_id  UUID           NOT NULL PRIMARY KEY,
    project_id   UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    agent_id     UUID           NOT NULL REFERENCES agent(agent_id) ON DELETE CASCADE,
    command_type VARCHAR(50)    NOT NULL,
    command_detail JSONB        NOT NULL,
    -- immutable execution payload: { cloud_account_id, deployment_id, provider,
    --   vm_identifier, region, resource_group, script_preview, command_args, ... }
    status       VARCHAR(20)    NOT NULL DEFAULT 'pending',
    requested_by UUID           NOT NULL REFERENCES dash_user(user_id),
    decided_by   UUID           REFERENCES dash_user(user_id),
    requested_at TIMESTAMPTZ    NOT NULL DEFAULT now(),
    decided_at   TIMESTAMPTZ,
    expires_at   TIMESTAMPTZ    NOT NULL,
    reason       TEXT
);
CREATE INDEX IF NOT EXISTS ix_command_approval_pending ON command_approval (project_id, status) WHERE status = 'pending';

-- 6. Test visibility rules
CREATE TABLE IF NOT EXISTS test_visibility_rule (
    rule_id       UUID           NOT NULL PRIMARY KEY,
    project_id    UUID           NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    user_id       UUID           REFERENCES dash_user(user_id) ON DELETE CASCADE,
    resource_type VARCHAR(20)    NOT NULL,
    resource_id   UUID           NOT NULL,
    created_by    UUID           NOT NULL REFERENCES dash_user(user_id),
    created_at    TIMESTAMPTZ    NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_visibility_project ON test_visibility_rule (project_id, user_id, resource_type);

-- 7. Add nullable project_id FK to all resource tables
ALTER TABLE agent ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE test_definition ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE job ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE schedule ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);
ALTER TABLE deployment ADD COLUMN IF NOT EXISTS project_id UUID REFERENCES project(project_id);

-- 8. Add cloud_account_id to deployment
ALTER TABLE deployment ADD COLUMN IF NOT EXISTS cloud_account_id UUID REFERENCES cloud_account(account_id);

-- 9. Add is_platform_admin to dash_user
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS is_platform_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- 10. Migrate existing admin users to platform admin
UPDATE dash_user SET is_platform_admin = TRUE WHERE role = 'admin';

-- 11. Create "Default" project (well-known UUID, idempotent)
INSERT INTO project (project_id, name, slug, description)
VALUES (
    '00000000-0000-0000-0000-000000000001',
    'Default',
    'default',
    'Auto-created during v0.15 migration. Contains all pre-existing resources.'
) ON CONFLICT DO NOTHING;

-- 12. Move all existing resources into Default project
UPDATE agent SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE test_definition SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE job SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE schedule SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;
UPDATE deployment SET project_id = '00000000-0000-0000-0000-000000000001' WHERE project_id IS NULL;

-- 13. Add all existing active users to Default project preserving current role
INSERT INTO project_member (project_id, user_id, role)
SELECT
    '00000000-0000-0000-0000-000000000001',
    user_id,
    CASE role
        WHEN 'admin' THEN 'admin'
        WHEN 'operator' THEN 'operator'
        ELSE 'viewer'
    END
FROM dash_user
WHERE status = 'active'
ON CONFLICT DO NOTHING;

-- 14. Project-scoped indexes on resource tables
CREATE INDEX IF NOT EXISTS ix_agent_project ON agent (project_id);
CREATE INDEX IF NOT EXISTS ix_test_def_project ON test_definition (project_id);
CREATE INDEX IF NOT EXISTS ix_job_project ON job (project_id, status, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_schedule_project ON schedule (project_id) WHERE enabled = TRUE;
CREATE INDEX IF NOT EXISTS ix_deployment_project ON deployment (project_id, status, created_at DESC);
```

- [ ] **Step 2: Register V010 in the `run()` function**

Follow the existing pattern in `run()`. Add after the V009 block:

```rust
// V010: Multi-project tenancy
let row = client
    .query_opt("SELECT version FROM _migrations WHERE version = 10", &[])
    .await?;

if row.is_none() {
    tracing::info!("Applying V010 multi-project migration...");
    client.batch_execute(V010_MULTI_PROJECT).await?;
    client
        .execute(
            "INSERT INTO _migrations (version) VALUES (10) ON CONFLICT DO NOTHING",
            &[],
        )
        .await?;
    tracing::info!("V010 migration complete");
}
```

- [ ] **Step 3: Verify migration is idempotent**

All `CREATE TABLE IF NOT EXISTS`, `ADD COLUMN IF NOT EXISTS`, `ON CONFLICT DO NOTHING`. Running the migration twice must be safe. The `UPDATE ... WHERE project_id IS NULL` statements are no-ops on second run.

- [ ] **Step 4: Build + test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
```

Test against a local PostgreSQL with existing v0.14 data to verify the migration runs cleanly and existing data is preserved.

- [ ] **Step 5: Commit PR 1**

```bash
git checkout -b feat/v015-schema-multi-project
git add crates/networker-dashboard/src/db/migrations.rs
git commit -m "feat(v0.15): V010 multi-project schema + Default project data migration

- New tables: project, project_member, cloud_account, share_link, command_approval, test_visibility_rule
- Nullable project_id FK added to agent, test_definition, job, schedule, deployment
- is_platform_admin flag on dash_user (migrates existing role='admin' users)
- Default project (UUID 000...001) created, all existing resources moved into it
- Existing active users added to Default project with their current role preserved
- Idempotent: safe to run multiple times"
```

---

## Task 2: Project CRUD + Membership API (PR 2)

**Branch:** `feat/v015-project-crud`
**Depends on:** PR 1

**Files:**
- Create: `crates/networker-dashboard/src/db/projects.rs`
- Create: `crates/networker-dashboard/src/api/projects.rs`
- Create: `crates/networker-dashboard/src/api/project_members.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/auth/mod.rs`
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `crates/networker-dashboard/src/db/users.rs`

- [ ] **Step 1: Add `is_platform_admin` to AuthUser and Claims**

In `auth/mod.rs`, update the `Claims` struct to include `is_platform_admin: bool`. Update `AuthUser` to include `is_platform_admin: bool`. Update `create_token` to accept and encode it. Update `require_auth` middleware to extract `is_platform_admin` from JWT claims and set it on `AuthUser`.

```rust
pub struct Claims {
    pub sub: Uuid,
    pub email: String,
    pub role: String,
    pub is_platform_admin: bool,  // NEW
    pub exp: usize,
    pub iat: usize,
}

pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
    pub role: String,
    pub is_platform_admin: bool,  // NEW
}
```

Update `create_token` signature:
```rust
pub fn create_token(
    user_id: Uuid,
    email: &str,
    role: &str,
    is_platform_admin: bool,  // NEW
    secret: &str,
) -> anyhow::Result<String>
```

Update all call sites of `create_token` in `api/auth.rs` (login, exchange-code, change-password) to pass `is_platform_admin`. Query it from `dash_user` table during authentication.

- [ ] **Step 2: Add ProjectRole enum and ProjectContext struct**

In `auth/mod.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectRole {
    Viewer,
    Operator,
    Admin,
}

impl ProjectRole {
    pub fn has_permission(&self, required: &ProjectRole) -> bool {
        self >= required
    }
}

#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub project_id: Uuid,
    pub project_slug: String,
    pub role: ProjectRole,
}
```

Note: `ProjectRole` derives `PartialOrd, Ord` with variants ordered Viewer < Operator < Admin, which makes `>=` comparison work for permission checks.

- [ ] **Step 3: Add `require_project` middleware**

In `auth/mod.rs`, add the middleware function. This extracts `:project_id` (UUID) from the URL path, checks project membership, and inserts `ProjectContext` into request extensions.

```rust
pub async fn require_project(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    // 1. Extract AuthUser from extensions (set by require_auth)
    let auth_user = match req.extensions().get::<AuthUser>().cloned() {
        Some(u) => u,
        None => return (StatusCode::UNAUTHORIZED, "Not authenticated").into_response(),
    };

    // 2. Extract project_id from path params
    // Parse the path to find the segment after "/projects/"
    let path = req.uri().path().to_string();
    let project_id = match extract_project_id_from_path(&path) {
        Some(id) => id,
        None => return (StatusCode::BAD_REQUEST, "Missing project ID in path").into_response(),
    };

    // 3. Fetch project
    let client = match state.db.get().await {
        Ok(c) => c,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    let project = match db::projects::get_project(&client, project_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return (StatusCode::NOT_FOUND, "Project not found").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    };

    // 4. Platform admin gets implicit admin access
    if auth_user.is_platform_admin {
        req.extensions_mut().insert(ProjectContext {
            project_id,
            project_slug: project.slug,
            role: ProjectRole::Admin,
        });
        return next.run(req).await;
    }

    // 5. Check membership
    match db::projects::get_member_role(&client, project_id, auth_user.user_id).await {
        Ok(Some(role)) => {
            req.extensions_mut().insert(ProjectContext {
                project_id,
                project_slug: project.slug,
                role,
            });
            next.run(req).await
        }
        Ok(None) => (StatusCode::FORBIDDEN, "Not a member of this project").into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response(),
    }
}

fn extract_project_id_from_path(path: &str) -> Option<Uuid> {
    let parts: Vec<&str> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == "projects" {
            if let Some(id_str) = parts.get(i + 1) {
                return id_str.parse::<Uuid>().ok();
            }
        }
    }
    None
}
```

- [ ] **Step 4: Add `require_project_role` helper**

```rust
pub fn require_project_role(ctx: &ProjectContext, required: ProjectRole) -> Result<(), StatusCode> {
    if ctx.role.has_permission(&required) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}
```

- [ ] **Step 5: Create `db/projects.rs`**

Add `pub mod projects;` to `db/mod.rs`.

Functions to implement:

```rust
pub struct ProjectRow {
    pub project_id: Uuid,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub settings: serde_json::Value,
}

pub struct ProjectMemberRow {
    pub project_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub joined_at: DateTime<Utc>,
    pub invited_by: Option<Uuid>,
    pub email: String,         // joined from dash_user
    pub display_name: Option<String>,  // joined from dash_user
}

// Project CRUD
pub async fn list_user_projects(client: &Client, user_id: Uuid, is_platform_admin: bool) -> Result<Vec<ProjectRow>>
// If platform admin: return all projects. Otherwise: join on project_member.
pub async fn get_project(client: &Client, project_id: Uuid) -> Result<Option<ProjectRow>>
pub async fn create_project(client: &Client, name: &str, slug: &str, description: Option<&str>, created_by: Uuid) -> Result<ProjectRow>
// Generate slug from name (lowercase, replace spaces with hyphens, strip non-alnum)
pub async fn update_project(client: &Client, project_id: Uuid, name: &str, description: Option<&str>, settings: serde_json::Value) -> Result<()>
pub async fn delete_project(client: &Client, project_id: Uuid) -> Result<()>
// Prevent deleting the Default project (UUID 000...001)

// Membership
pub async fn get_member_role(client: &Client, project_id: Uuid, user_id: Uuid) -> Result<Option<ProjectRole>>
pub async fn list_members(client: &Client, project_id: Uuid) -> Result<Vec<ProjectMemberRow>>
pub async fn add_member(client: &Client, project_id: Uuid, user_id: Uuid, role: &str, invited_by: Uuid) -> Result<()>
pub async fn update_member_role(client: &Client, project_id: Uuid, user_id: Uuid, role: &str) -> Result<()>
pub async fn remove_member(client: &Client, project_id: Uuid, user_id: Uuid) -> Result<()>
// Prevent removing the last admin from a project

// Slug helper
pub fn slugify(name: &str) -> String
```

- [ ] **Step 6: Create `api/projects.rs`**

Endpoints (all under `/api/projects`):

```
GET  /api/projects                    — list_projects (any authenticated user)
POST /api/projects                    — create_project (platform admin only)
GET  /api/projects/:pid               — get_project (project member or platform admin)
PUT  /api/projects/:pid               — update_project (project admin)
DELETE /api/projects/:pid             — delete_project (platform admin only)
```

The `list_projects` handler calls `db::projects::list_user_projects` with the user's ID and `is_platform_admin` flag. Returns `Vec<{ project_id, name, slug, role, description }>` where `role` is the user's role in each project.

The `create_project` handler:
1. Requires `is_platform_admin`
2. Generates slug from name
3. Creates the project
4. Adds the creating user as project admin

- [ ] **Step 7: Create `api/project_members.rs`**

Endpoints (all under `/api/projects/:pid/members`, require `require_project` middleware):

```
GET    /api/projects/:pid/members           — list_members (project admin)
POST   /api/projects/:pid/members           — add_member (project admin)
PUT    /api/projects/:pid/members/:uid      — update_member_role (project admin)
DELETE /api/projects/:pid/members/:uid      — remove_member (project admin)
```

`add_member` body: `{ email: string, role: "admin" | "operator" | "viewer" }`. Look up user by email in `dash_user`. If not found, return 404 with message suggesting invitation (v0.14 invite flow).

- [ ] **Step 8: Register project routes in `api/mod.rs`**

Add `mod projects;` and `mod project_members;` to `api/mod.rs`.

Register the project list/create routes under the `protected` router (require_auth only, no project context).

Register the project detail/members routes under a new `project_scoped` sub-router that applies both `require_auth` and `require_project` middleware.

```rust
// In api/mod.rs router():
let project_scoped = Router::new()
    .merge(project_members::router(state.clone()))
    // ... more project-scoped routes added in later PRs
    .layer(middleware::from_fn_with_state(
        state.clone(),
        crate::auth::require_project,
    ))
    .layer(middleware::from_fn_with_state(
        state.clone(),
        crate::auth::require_auth,
    ));

// Nest under /api/projects/:project_id
let project_nested = Router::new()
    .nest("/api/projects/:project_id", project_scoped);
```

Also add the flat project CRUD routes (`/api/projects`) to the protected router.

- [ ] **Step 9: Update `db/users.rs` to query `is_platform_admin`**

Update `authenticate` and other user-fetching queries to SELECT `is_platform_admin` from `dash_user`. Return it alongside email, role, etc.

- [ ] **Step 10: Update `api/auth.rs` login/SSO to pass `is_platform_admin`**

When issuing JWTs in the login handler, exchange-code handler, and change-password handler, fetch `is_platform_admin` from DB and pass to `create_token`.

Update `LoginResponse` to include `is_platform_admin: bool`.

- [ ] **Step 11: Build + test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
```

Test: create a project, list projects, add/remove members. Test that platform admin can access all projects. Test that non-member gets 403.

- [ ] **Step 12: Commit PR 2**

```bash
git checkout -b feat/v015-project-crud
git add -A
git commit -m "feat(v0.15): project CRUD + membership API + require_project middleware

- ProjectContext middleware: extracts project_id from URL, checks membership
- Platform admin gets implicit admin access to all projects
- ProjectRole enum with Viewer < Operator < Admin ordering
- is_platform_admin added to JWT claims and AuthUser
- Project CRUD: list, create, get, update, delete
- Member management: list, add, update role, remove
- Slug generation from project name
- Safety: prevent deleting Default project, prevent removing last admin"
```

---

## Task 3: Project-Scoped Resource APIs + Backward Compat (PR 3)

**Branch:** `feat/v015-scoped-resources`
**Depends on:** PR 2

**Files:**
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/api/agents.rs`
- Modify: `crates/networker-dashboard/src/api/jobs.rs`
- Modify: `crates/networker-dashboard/src/api/runs.rs`
- Modify: `crates/networker-dashboard/src/api/url_tests.rs`
- Modify: `crates/networker-dashboard/src/api/schedules.rs`
- Modify: `crates/networker-dashboard/src/api/deployments.rs`
- Modify: `crates/networker-dashboard/src/api/dashboard.rs`
- Modify: `crates/networker-dashboard/src/api/cloud.rs`
- Modify: `crates/networker-dashboard/src/api/inventory.rs`
- Modify: `crates/networker-dashboard/src/api/cloud_connections.rs`
- Modify: `crates/networker-dashboard/src/db/agents.rs`
- Modify: `crates/networker-dashboard/src/db/jobs.rs`
- Modify: `crates/networker-dashboard/src/db/runs.rs`
- Modify: `crates/networker-dashboard/src/db/url_tests.rs`
- Modify: `crates/networker-dashboard/src/db/schedules.rs`
- Modify: `crates/networker-dashboard/src/db/deployments.rs`
- Modify: `crates/networker-dashboard/src/db/cloud_connections.rs`
- Modify: `crates/networker-dashboard/src/ws/agent_hub.rs`
- Modify: `crates/networker-dashboard/src/ws/browser_hub.rs`

This is the largest PR. The core pattern is the same for every resource: add `project_id` parameter to DB queries and handler extraction.

- [ ] **Step 1: Update all DB query functions to accept `project_id`**

For each DB module (`agents.rs`, `jobs.rs`, `runs.rs`, `url_tests.rs`, `schedules.rs`, `deployments.rs`, `cloud_connections.rs`):

1. Add `project_id: Uuid` parameter to all list/create/get functions
2. Add `WHERE project_id = $N` to all SELECT queries
3. Add `project_id` to INSERT statements
4. For `runs.rs`: runs belong to jobs which belong to projects. Add a join or subquery: `WHERE j.project_id = $1` when listing runs.
5. For `url_tests.rs`: add project scoping before exposing the routes. Either add `project_id` to the URL-test tables in the same migration family, or create a durable mapping table keyed by `UrlTestRun.Id` and filter every URL-test query through it.

Example pattern for `db/agents.rs`:

```rust
// Before:
pub async fn list_agents(client: &Client) -> anyhow::Result<Vec<AgentRow>> {
    let rows = client.query("SELECT ... FROM agent ORDER BY ...", &[]).await?;
    ...
}

// After:
pub async fn list_agents(client: &Client, project_id: Uuid) -> anyhow::Result<Vec<AgentRow>> {
    let rows = client.query(
        "SELECT ... FROM agent WHERE project_id = $1 ORDER BY ...",
        &[&project_id],
    ).await?;
    ...
}
```

Apply this pattern to:
- `db/agents.rs`: `list_agents`, `get_agent`, `register_agent`, `delete_agent`
- `db/jobs.rs`: `list_jobs`, `create_job`, `get_job`, `cancel_job`
- `db/runs.rs`: `list_runs`, `get_run` (join through job.project_id)
- `db/url_tests.rs`: `list`, `get`, `section_detail` backing queries
- `db/schedules.rs`: `list_schedules`, `create_schedule`, `update_schedule`, `delete_schedule`
- `db/deployments.rs`: `list_deployments`, `create_deployment`, `get_deployment`
- `db/cloud_connections.rs`: `list_connections`, `create_connection`, `get_connection`, `update_connection`, `delete_connection`

- [ ] **Step 2: Update all API handlers to extract `ProjectContext`**

For each handler in the project-scoped route group, extract `ProjectContext` from request extensions:

```rust
async fn list_agents(
    Extension(auth): Extension<AuthUser>,
    Extension(ctx): Extension<ProjectContext>,  // NEW
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let client = state.db.get().await?;
    let agents = db::agents::list_agents(&client, ctx.project_id).await?;  // CHANGED
    Json(agents)
}
```

For mutation endpoints, also add role checks:
```rust
async fn create_job(
    Extension(auth): Extension<AuthUser>,
    Extension(ctx): Extension<ProjectContext>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateJobRequest>,
) -> impl IntoResponse {
    require_project_role(&ctx, ProjectRole::Operator)?;  // NEW
    ...
}
```

Role requirements:
- GET endpoints: any project member (Viewer+)
- POST/PUT/DELETE for jobs, schedules, deployments: Operator+
- Agent registration/deletion: Operator+
- Dashboard summary: Viewer+

- [ ] **Step 3: Create project-scoped router in `api/mod.rs`**

Restructure `api/mod.rs` to have both the old flat routes AND new project-scoped routes:

```rust
pub fn router(state: Arc<AppState>) -> Router {
    let public = Router::new().merge(auth::router(state.clone()));

    // Flat protected routes (require_auth only — kept as temporary aliases so
    // the current frontend can continue to call flat detail/action endpoints
    // until PR 4 lands)
    let protected_flat = Router::new()
        .merge(auth::protected_router(state.clone()))
        .merge(agents::router(state.clone()))
        .merge(jobs::router(state.clone()))
        .merge(runs::router(state.clone()))
        .merge(url_tests::router(state.clone()))
        .merge(schedules::router(state.clone()))
        .merge(dashboard::router(state.clone()))
        .merge(deployments::router(state.clone()))
        .merge(cloud::router(state.clone()))
        .merge(cloud_connections::router(state.clone()))
        .merge(inventory::router(state.clone()))
        .merge(update::router(state.clone()))
        .merge(modes::router(state.clone()))
        .merge(version::router(state.clone()))
        .merge(users::router(state.clone()))
        .merge(projects::router(state.clone()))  // /api/projects list+create
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    // Project-scoped routes (require_auth + require_project)
    let project_scoped = Router::new()
        .merge(agents::project_router(state.clone()))
        .merge(jobs::project_router(state.clone()))
        .merge(runs::project_router(state.clone()))
        .merge(schedules::project_router(state.clone()))
        .merge(dashboard::project_router(state.clone()))
        .merge(deployments::project_router(state.clone()))
        .merge(cloud::project_router(state.clone()))
        .merge(cloud_connections::project_router(state.clone()))
        .merge(inventory::project_router(state.clone()))
        .merge(url_tests::project_router(state.clone()))
        .merge(project_members::router(state.clone()))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_project,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::require_auth,
        ));

    let project_nested = Router::new()
        .nest("/api/projects/:project_id", project_scoped);

    public
        .merge(protected_flat)
        .merge(project_nested)
}
```

- [ ] **Step 4: Keep flat API aliases targeting the Default project**

Do not use HTTP redirects for API compatibility. Keep the existing flat routers mounted temporarily and make their handlers call the same shared implementation as the project-scoped routes, with the Default project ID injected explicitly. This preserves existing collection, detail, and action endpoints used by the current UI while PR 4 is still outstanding.

```rust
const DEFAULT_PROJECT_ID: Uuid = uuid::uuid!("00000000-0000-0000-0000-000000000001");

async fn list_jobs_flat(
    Extension(auth): Extension<AuthUser>,
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListJobsQuery>,
) -> impl IntoResponse {
    list_jobs_impl(auth, ProjectScope::DefaultProject(DEFAULT_PROJECT_ID), state, q).await
}
```

Cover all currently-used flat routes, not just list endpoints:
- `/api/agents`, `/api/agents/:agent_id`
- `/api/jobs`, `/api/jobs/:job_id`, `/api/jobs/:job_id/cancel`
- `/api/runs`, `/api/runs/:run_id`, `/api/runs/:run_id/attempts`
- `/api/url-tests`, `/api/url-tests/:run_id`, `/api/url-tests/:run_id/sections`
- `/api/deployments`, `/api/deployments/:deployment_id`, `/api/deployments/:deployment_id/stop`, `/api/deployments/:deployment_id/check`, `/api/deployments/:deployment_id/update`
- `/api/schedules`, `/api/schedules/:schedule_id`, plus existing schedule action routes
- `/api/dashboard/summary`

- [ ] **Step 5: Update each resource API module to expose `project_router`**

In each module (agents.rs, jobs.rs, etc.), add a `project_router` function alongside the existing `router`. The `project_router` uses paths relative to the nested mount point (e.g., `/agents` instead of `/api/agents`):

```rust
// In api/agents.rs:
pub fn project_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/agents", axum::routing::get(list_agents))
        .route("/agents/:agent_id", axum::routing::get(get_agent).delete(delete_agent))
        .with_state(state)
}
```

Keep the old `router()` function temporarily as the flat compatibility layer. Do not reduce it to collection-only redirects. Remove it in PR 10 after the frontend and bookmarks have soaked on project-scoped URLs.

- [ ] **Step 6: Update WebSocket hubs for project scoping**

In `ws/agent_hub.rs`:
- Agent registration includes `project_id`
- Agents are stored keyed by `(project_id, agent_id)`
- When sending jobs to agents, filter by project_id
- When an agent submits results, validate it belongs to the job's project

In `ws/browser_hub.rs`:
- Browser WS connections include a project_id query parameter: `/ws/dashboard?project_id=UUID`
- Events are broadcast only to browsers watching the same project
- This filters test results, agent status changes, deployment events

- [ ] **Step 7: Update `deploy/runner.rs` for project context**

Add `project_id: Uuid` to `DeployRequest` or extract it from the deployment record. Deployment runner reads project_id to ensure agents are registered in the correct project.

- [ ] **Step 8: Build + integration test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
```

Test: old flat routes still succeed against the Default project, including detail/action endpoints. Test: project-scoped routes return only project resources, including URL tests. Test: WebSocket events are project-scoped.

- [ ] **Step 9: Commit PR 3**

```bash
git checkout -b feat/v015-scoped-resources
git add -A
git commit -m "feat(v0.15): project-scoped resource APIs + flat compatibility aliases

- All resource APIs now live under /api/projects/:pid/
- DB queries filter by project_id
- Role checks: Viewer for reads, Operator for mutations
- Flat compatibility aliases keep existing UI/API clients working against Default project
- URL-test APIs are brought under project scoping
- WebSocket hubs filter events by project_id
- Agent registration includes project_id"
```

---

## Task 4: Frontend Project Switcher + Scoped Routes (PR 4)

**Branch:** `feat/v015-frontend-projects`
**Depends on:** PR 3

**Files:**
- Create: `dashboard/src/stores/projectStore.ts`
- Create: `dashboard/src/hooks/useProject.ts`
- Create: `dashboard/src/components/ProjectSwitcher.tsx`
- Create: `dashboard/src/pages/ProjectsPage.tsx`
- Create: `dashboard/src/pages/ProjectSettingsPage.tsx`
- Create: `dashboard/src/pages/ProjectMembersPage.tsx`
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/stores/authStore.ts`
- Modify: `dashboard/src/components/layout/Sidebar.tsx`
- Modify: `dashboard/src/hooks/useWebSocket.ts`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`
- Modify: `dashboard/src/pages/DashboardPage.tsx`
- Modify: `dashboard/src/pages/JobsPage.tsx`
- Modify: `dashboard/src/pages/JobDetailPage.tsx`
- Modify: `dashboard/src/pages/RunsPage.tsx`
- Modify: `dashboard/src/pages/RunDetailPage.tsx`
- Modify: `dashboard/src/pages/DeployPage.tsx`
- Modify: `dashboard/src/pages/DeployDetailPage.tsx`
- Modify: `dashboard/src/pages/SchedulesPage.tsx`
- Modify: `dashboard/src/pages/SettingsPage.tsx`

- [ ] **Step 1: Update `authStore.ts`**

Add `isPlatformAdmin` field:

```typescript
interface AuthState {
  token: string | null;
  email: string | null;
  role: string | null;
  status: string | null;
  isPlatformAdmin: boolean;  // NEW
  isAuthenticated: boolean;
  mustChangePassword: boolean;
  login: (token: string, email: string, role: string, mustChangePassword?: boolean, status?: string, isPlatformAdmin?: boolean) => void;
  updateStatus: (status: string) => void;
  clearPasswordChange: () => void;
  logout: () => void;
}
```

Persist `isPlatformAdmin` in localStorage. Update `login()` to accept and store it. Update `logout()` to clear it.

- [ ] **Step 2: Create `projectStore.ts`**

```typescript
import { create } from 'zustand';

interface ProjectSummary {
  project_id: string;
  name: string;
  slug: string;
  role: string;  // user's role in this project
  description?: string;
}

interface ProjectState {
  projects: ProjectSummary[];
  activeProjectId: string | null;
  activeProjectSlug: string | null;
  activeProjectRole: string | null;

  setProjects: (projects: ProjectSummary[]) => void;
  setActiveProject: (project: ProjectSummary) => void;
  clearActiveProject: () => void;
  clear: () => void;
}

export const useProjectStore = create<ProjectState>((set) => ({
  projects: [],
  activeProjectId: localStorage.getItem('activeProjectId'),
  activeProjectSlug: localStorage.getItem('activeProjectSlug'),
  activeProjectRole: localStorage.getItem('activeProjectRole'),

  setProjects: (projects) => set({ projects }),

  setActiveProject: (project) => {
    localStorage.setItem('activeProjectId', project.project_id);
    localStorage.setItem('activeProjectSlug', project.slug);
    localStorage.setItem('activeProjectRole', project.role);
    set({
      activeProjectId: project.project_id,
      activeProjectSlug: project.slug,
      activeProjectRole: project.role,
    });
  },

  clearActiveProject: () => {
    localStorage.removeItem('activeProjectId');
    localStorage.removeItem('activeProjectSlug');
    localStorage.removeItem('activeProjectRole');
    set({ activeProjectId: null, activeProjectSlug: null, activeProjectRole: null });
  },

  clear: () => {
    localStorage.removeItem('activeProjectId');
    localStorage.removeItem('activeProjectSlug');
    localStorage.removeItem('activeProjectRole');
    set({ projects: [], activeProjectId: null, activeProjectSlug: null, activeProjectRole: null });
  },
}));
```

- [ ] **Step 3: Create `useProject.ts` hook**

```typescript
import { useParams, useNavigate } from 'react-router-dom';
import { useProjectStore } from '../stores/projectStore';
import { useEffect } from 'react';

export function useProject() {
  const { projectId } = useParams<{ projectId: string }>();
  const navigate = useNavigate();
  const { projects, activeProjectId, setActiveProject } = useProjectStore();

  // Sync URL project with store
  useEffect(() => {
    if (projectId && projectId !== activeProjectId) {
      const project = projects.find(p => p.project_id === projectId);
      if (project) {
        setActiveProject(project);
      }
    }
  }, [projectId, activeProjectId, projects, setActiveProject]);

  return {
    projectId: projectId || activeProjectId,
    projectRole: useProjectStore(s => s.activeProjectRole),
    isProjectAdmin: useProjectStore(s => s.activeProjectRole) === 'admin',
    isOperator: useProjectStore(s => s.activeProjectRole) === 'admin' || useProjectStore(s => s.activeProjectRole) === 'operator',
  };
}
```

- [ ] **Step 4: Update `api/client.ts`**

All resource API calls now include the project_id prefix. Add a helper:

```typescript
function projectUrl(projectId: string, path: string): string {
  return `/api/projects/${projectId}/${path}`;
}
```

Update all existing methods to accept `projectId` as first parameter:
- `getAgents(projectId)` -> `GET /api/projects/${pid}/agents`
- `getJobs(projectId)` -> `GET /api/projects/${pid}/jobs`
- `createJob(projectId, config)` -> `POST /api/projects/${pid}/jobs`
- etc.

Add new project API methods:
```typescript
getProjects(): Promise<ProjectSummary[]>
createProject(name, description?): Promise<ProjectRow>
getProject(projectId): Promise<ProjectRow>
updateProject(projectId, name, description, settings): Promise<void>
deleteProject(projectId): Promise<void>
getProjectMembers(projectId): Promise<MemberRow[]>
addProjectMember(projectId, email, role): Promise<void>
updateMemberRole(projectId, userId, role): Promise<void>
removeProjectMember(projectId, userId): Promise<void>
```

- [ ] **Step 5: Create `ProjectSwitcher.tsx`**

Dropdown component for the sidebar header:

```
┌─────────────────────────┐
│ ▾ Production Network     │  <- click to open dropdown
│   ○ connected            │
└─────────────────────────┘
```

When open, shows:
- List of user's projects with role badge (admin=green, operator=cyan, viewer=gray)
- Divider
- "Create Project" link (only if `isPlatformAdmin`)

Clicking a project navigates to `/projects/:pid` and updates projectStore.

Style: dark dropdown, `bg-[var(--bg-card)]`, border `border-gray-800`, each item `hover:bg-gray-800/50`. Role badges use same color scheme as UsersPage.

- [ ] **Step 6: Update `Sidebar.tsx`**

Replace the fixed "AletheDash" / connection dot header with `<ProjectSwitcher />`.

Add new nav items after the divider:
```typescript
const projectNavItems = [
  { path: `/projects/${pid}/cloud-accounts`, label: 'Cloud Accounts', icon: '☁', minRole: 'operator' },
  { path: `/projects/${pid}/share-links`, label: 'Share Links', icon: '🔗', minRole: 'admin' },
];

const projectAdminItems = [
  { path: `/projects/${pid}/members`, label: 'Members', icon: '◈', minRole: 'admin' },
  { path: `/projects/${pid}/settings`, label: 'Project Settings', icon: '⚙', minRole: 'admin' },
];

const platformAdminItems = [
  { path: '/users', label: 'Users', icon: '◉', platformAdmin: true },
];
```

Update all existing nav item paths to include the project prefix:
```typescript
const navItems = [
  { path: `/projects/${pid}`, label: 'Dashboard', icon: '◈' },
  { path: `/projects/${pid}/deploy`, label: 'Infra', icon: '▣' },
  { path: `/projects/${pid}/tests`, label: 'Tests', icon: '▶' },
  { path: `/projects/${pid}/schedules`, label: 'Schedules', icon: '↻' },
  { path: `/projects/${pid}/runs`, label: 'Runs', icon: '◷' },
];
```

- [ ] **Step 7: Update `App.tsx` route structure**

Replace the flat routes with project-scoped routes:

```tsx
function AuthenticatedApp() {
  // ... existing status/password checks ...

  return (
    <div className="flex min-h-screen bg-[var(--bg-base)]">
      <Sidebar connectionDot={<ConnectionDot status={status} />} />
      <main className="flex-1 overflow-auto pt-12 md:pt-0">
        <ConnectionBanner status={status} />
        <ToastContainer />
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
          <Route path="/projects/:projectId/deploy/:deploymentId" element={<DeployDetailPage />} />
          <Route path="/projects/:projectId/schedules" element={<SchedulesPage />} />
          <Route path="/projects/:projectId/settings" element={<ProjectSettingsPage />} />
          <Route path="/projects/:projectId/members" element={<ProjectMembersPage />} />

          {/* Platform admin routes */}
          <Route path="/users" element={<UsersPage />} />
          <Route path="/change-password" element={<ChangePasswordPage />} />
          <Route path="/pending" element={<PendingPage />} />

          {/* Redirect root to last active project or project list */}
          <Route path="/" element={<ProjectRedirect />} />

          {/* Backward compat redirects (old flat routes) */}
          {/* LegacyRedirect must preserve any route params so detail bookmarks
              land on the corresponding project-scoped detail page. */}
          <Route path="/tests" element={<LegacyRedirect to="tests" />} />
          <Route path="/tests/:jobId" element={<LegacyRedirect to="tests" preserveParam="jobId" />} />
          <Route path="/runs" element={<LegacyRedirect to="runs" />} />
          <Route path="/runs/:runId" element={<LegacyRedirect to="runs" preserveParam="runId" />} />
          <Route path="/deploy" element={<LegacyRedirect to="deploy" />} />
          <Route path="/deploy/:deploymentId" element={<LegacyRedirect to="deploy" preserveParam="deploymentId" />} />
          <Route path="/schedules" element={<LegacyRedirect to="schedules" />} />
          <Route path="/settings" element={<LegacyRedirect to="settings" />} />
          <Route path="*" element={<Navigate to="/" />} />
        </Routes>
      </main>
    </div>
  );
}
```

Add helper components:

```tsx
function ProjectRedirect() {
  const activeProjectId = useProjectStore(s => s.activeProjectId);
  if (activeProjectId) return <Navigate to={`/projects/${activeProjectId}`} />;
  return <Navigate to="/projects" />;
}

function LegacyRedirect({ to }: { to: string }) {
  const activeProjectId = useProjectStore(s => s.activeProjectId);
  if (activeProjectId) return <Navigate to={`/projects/${activeProjectId}/${to}`} />;
  return <Navigate to="/projects" />;
}
```

- [ ] **Step 8: Update all page components to use `useProject()`**

In every page component (DashboardPage, JobsPage, JobDetailPage, RunsPage, RunDetailPage, DeployPage, DeployDetailPage, SchedulesPage, SettingsPage):

1. Import and call `useProject()` at the top
2. Pass `projectId` to all API calls
3. Update any internal links to include the project prefix

Example for `JobsPage.tsx`:
```typescript
export function JobsPage() {
  const { projectId } = useProject();
  // Change: api.getJobs() → api.getJobs(projectId)
  // Change: navigate('/tests/123') → navigate(`/projects/${projectId}/tests/123`)
  ...
}
```

- [ ] **Step 9: Create `ProjectsPage.tsx`**

Grid layout showing user's projects:

```
┌──────────────────────────────────────────────────┐
│  Projects                    [+ Create Project]  │
├──────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐              │
│  │ Default       │  │ Staging      │              │
│  │ 3 agents      │  │ 1 agent      │              │
│  │ admin ●       │  │ operator ●   │              │
│  └──────────────┘  └──────────────┘              │
└──────────────────────────────────────────────────┘
```

On mount: fetch `api.getProjects()`, store in projectStore. Click card navigates to `/projects/:pid`. "Create Project" button only for `isPlatformAdmin` -- opens inline form (name + description).

- [ ] **Step 10: Create `ProjectSettingsPage.tsx`**

Project admin page with:
- Project name (editable)
- Project description (editable)
- Project slug (read-only, generated from name)
- Test visibility setting: dropdown "All viewers can see all tests" / "Explicit visibility rules"
- Danger zone: Delete project (platform admin only, cannot delete Default project)

- [ ] **Step 11: Create `ProjectMembersPage.tsx`**

Table of members:
```
┌──────────────────────────────────────────────────┐
│  Members                       [+ Add Member]    │
├──────────────────────────────────────────────────┤
│  alice@company.com    admin     ●   [Remove]     │
│  bob@company.com      operator  ●   [Remove]     │
│  carol@company.com    viewer    ●   [Remove]     │
└──────────────────────────────────────────────────┘
```

Role is a dropdown selector (admin/operator/viewer). "Add Member" opens a dialog: email input + role selector. "Remove" button with confirmation. Cannot remove yourself. Cannot remove the last admin.

- [ ] **Step 12: Update `useWebSocket.ts`**

Add project_id to WebSocket URL:
```typescript
const wsUrl = projectId
  ? `${wsBase}/ws/dashboard?token=${token}&project_id=${projectId}`
  : `${wsBase}/ws/dashboard?token=${token}`;
```

Reconnect when project changes.

- [ ] **Step 13: Fetch projects on login and store**

In the `LoginPage.tsx` (and `SSOCompletePage.tsx`) success path, after `authStore.login(...)`:
1. Fetch `api.getProjects()`
2. Store in `projectStore.setProjects(projects)`
3. If `projects.length === 1`, auto-select it and navigate to `/projects/:pid`
4. If multiple projects, navigate to `/projects` (list)

Also fetch projects on app mount (in `AuthenticatedApp` useEffect) to handle page refresh.

- [ ] **Step 14: Build + test**

```bash
cd dashboard && npm run build && npm run lint
```

Test: login redirects to single project. Project switcher shows all projects. Switching projects updates URL and API calls. Old URLs redirect while preserving detail IDs. Members page works.

- [ ] **Step 15: Commit PR 4**

```bash
git checkout -b feat/v015-frontend-projects
git add -A
git commit -m "feat(v0.15): frontend project switcher + scoped routes + members/settings pages

- ProjectSwitcher dropdown in sidebar
- All routes now under /projects/:projectId/
- projectStore (Zustand) for active project context
- useProject() hook for reading project from URL
- ProjectsPage: grid of user's projects
- ProjectSettingsPage: name, description, visibility
- ProjectMembersPage: manage members and roles
- All API calls pass projectId
- WebSocket reconnects on project change
- Legacy route redirects to active project"
```

---

## Task 5: Cloud Account Encryption + API (PR 5)

**Branch:** `feat/v015-cloud-accounts`
**Depends on:** PR 3

**Files:**
- Create: `crates/networker-dashboard/src/crypto.rs`
- Create: `crates/networker-dashboard/src/api/cloud_accounts.rs`
- Create: `crates/networker-dashboard/src/db/cloud_accounts.rs`
- Modify: `crates/networker-dashboard/Cargo.toml`
- Modify: `crates/networker-dashboard/src/config.rs`
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`

- [ ] **Step 1: Add `aes-gcm` dependency**

In `crates/networker-dashboard/Cargo.toml`, add:
```toml
aes-gcm  = "0.10"
```

The `sha2`, `rand`, and `base64` crates are already present.

- [ ] **Step 2: Add credential key to config**

In `config.rs`, add to `DashboardConfig`:
```rust
pub credential_key: Option<[u8; 32]>,
pub credential_key_old: Option<[u8; 32]>,
pub share_base_url: String,
pub share_max_days: u32,
```

In `from_env()`:
```rust
let credential_key = std::env::var("DASHBOARD_CREDENTIAL_KEY")
    .ok()
    .filter(|s| s.len() == 64)
    .and_then(|s| hex::decode(&s).ok())
    .and_then(|v| <[u8; 32]>::try_from(v).ok());

let credential_key_old = std::env::var("DASHBOARD_CREDENTIAL_KEY_OLD")
    .ok()
    .filter(|s| s.len() == 64)
    .and_then(|s| hex::decode(&s).ok())
    .and_then(|v| <[u8; 32]>::try_from(v).ok());

let share_base_url = std::env::var("DASHBOARD_SHARE_URL")
    .unwrap_or_else(|_| public_url.clone());

let share_max_days: u32 = std::env::var("DASHBOARD_SHARE_MAX_DAYS")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(365);
```

Note: need to add `hex = "0.4"` to Cargo.toml for hex decoding, OR use the existing `base64` crate if keys are stored as base64. Prefer hex for clarity since the spec says "64 hex chars".

Add `hex = "0.4"` to `Cargo.toml`.

- [ ] **Step 3: Update `AppState` in `main.rs`**

Add new fields to `AppState`:
```rust
pub credential_key: Option<[u8; 32]>,
pub credential_key_old: Option<[u8; 32]>,
pub share_base_url: String,
pub share_max_days: u32,
```

Initialize from config in `main()`.

- [ ] **Step 4: Create `crypto.rs`**

```rust
use aes_gcm::{Aes256Gcm, Key, Nonce, aead::Aead, KeyInit};
use rand::RngCore;

/// Encrypt plaintext with AES-256-GCM. Returns (ciphertext, 12-byte nonce).
pub fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> anyhow::Result<(Vec<u8>, [u8; 12])> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("Encryption failed: {e}"))?;
    Ok((ciphertext, nonce_bytes))
}

/// Decrypt ciphertext with AES-256-GCM.
pub fn decrypt(ciphertext: &[u8], nonce: &[u8; 12], key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {e}"))
}

/// Try to decrypt with primary key, fall back to old key (for rotation).
pub fn decrypt_with_fallback(
    ciphertext: &[u8],
    nonce: &[u8; 12],
    key: &[u8; 32],
    old_key: Option<&[u8; 32]>,
) -> anyhow::Result<Vec<u8>> {
    match decrypt(ciphertext, nonce, key) {
        Ok(v) => Ok(v),
        Err(_) if old_key.is_some() => decrypt(ciphertext, nonce, old_key.unwrap()),
        Err(e) => Err(e),
    }
}
```

Add `mod crypto;` to `main.rs`.

- [ ] **Step 5: Create `db/cloud_accounts.rs`**

Add `pub mod cloud_accounts;` to `db/mod.rs`.

```rust
pub struct CloudAccountRow {
    pub account_id: Uuid,
    pub project_id: Uuid,
    pub owner_id: Option<Uuid>,
    pub name: String,
    pub provider: String,
    pub credentials_enc: Vec<u8>,
    pub credentials_nonce: Vec<u8>,
    pub region_default: Option<String>,
    pub status: String,
    pub last_validated: Option<DateTime<Utc>>,
    pub validation_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CloudAccountSummary {
    pub account_id: Uuid,
    pub name: String,
    pub provider: String,
    pub region_default: Option<String>,
    pub personal: bool,
    pub status: String,
    pub last_validated: Option<DateTime<Utc>>,
}

// List: project-shared accounts + requesting user's personal accounts
pub async fn list_accounts(client: &Client, project_id: Uuid, user_id: Uuid) -> Result<Vec<CloudAccountSummary>>
// SELECT ... FROM cloud_account WHERE project_id = $1 AND (owner_id IS NULL OR owner_id = $2)

pub async fn get_account(client: &Client, account_id: Uuid, project_id: Uuid) -> Result<Option<CloudAccountRow>>

pub async fn create_account(
    client: &Client,
    project_id: Uuid,
    owner_id: Option<Uuid>,
    name: &str,
    provider: &str,
    credentials_enc: &[u8],
    credentials_nonce: &[u8],
    region_default: Option<&str>,
) -> Result<Uuid>

pub async fn update_account(client: &Client, account_id: Uuid, name: &str, region_default: Option<&str>) -> Result<()>

pub async fn delete_account(client: &Client, account_id: Uuid, project_id: Uuid) -> Result<()>

pub async fn update_validation(client: &Client, account_id: Uuid, status: &str, error: Option<&str>) -> Result<()>
```

- [ ] **Step 6: Create `api/cloud_accounts.rs`**

Add `mod cloud_accounts;` to `api/mod.rs`.

Endpoints (all project-scoped):

```
GET    /cloud-accounts            — list accounts (viewer+)
POST   /cloud-accounts            — create account (operator+ for personal, admin for shared)
GET    /cloud-accounts/:aid       — get account metadata (viewer+, never return credentials)
PUT    /cloud-accounts/:aid       — update name/region (admin for shared, owner for personal)
DELETE /cloud-accounts/:aid       — delete (admin for shared, owner for personal)
POST   /cloud-accounts/:aid/validate — test credentials (operator+)
```

**Create handler logic:**
1. If `DASHBOARD_CREDENTIAL_KEY` not configured, return 503 "Cloud accounts require DASHBOARD_CREDENTIAL_KEY"
2. Parse request body: `{ name, provider, credentials: {...}, region_default, personal: bool }`
3. If `personal: true`: set `owner_id` to requesting user. Require Operator+.
4. If `personal: false`: require Project Admin.
5. Serialize `credentials` to JSON bytes
6. Encrypt with `crypto::encrypt(json_bytes, &state.credential_key)`
7. Store encrypted ciphertext + nonce in DB
8. Return account metadata (never credentials)

**Validate handler logic:**
1. Fetch account row (including encrypted credentials)
2. Decrypt credentials
3. Based on provider, run a lightweight validation call:
   - Azure: `az account show` equivalent via REST API with the service principal
   - AWS: `sts get-caller-identity` via REST API with the access keys
   - GCP: list projects with the service account key
4. Update `last_validated` and `status` in DB
5. Return validation result

- [ ] **Step 7: Register in `api/mod.rs`**

Add `cloud_accounts::project_router(state.clone())` to the `project_scoped` router in `api/mod.rs`.

- [ ] **Step 8: Build + test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
```

Test: create account without key returns 503. Create with key encrypts. List returns metadata only. Validate decrypts and tests credentials.

- [ ] **Step 9: Commit PR 5**

```bash
git checkout -b feat/v015-cloud-accounts
git add -A
git commit -m "feat(v0.15): cloud account encryption + API

- AES-256-GCM encryption for cloud credentials at rest
- DASHBOARD_CREDENTIAL_KEY env var (64 hex chars)
- Key rotation support via DASHBOARD_CREDENTIAL_KEY_OLD
- Cloud account CRUD: project-shared + personal accounts
- Credential validation endpoint (Azure/AWS/GCP)
- Credentials never returned in API responses
- 503 if encryption key not configured"
```

---

## Task 6: Frontend Cloud Accounts + Deploy Wizard Integration (PR 6)

**Branch:** `feat/v015-frontend-cloud-accounts`
**Depends on:** PR 4, PR 5

**Files:**
- Create: `dashboard/src/pages/CloudAccountsPage.tsx`
- Create: `dashboard/src/components/CloudAccountSelector.tsx`
- Modify: `dashboard/src/components/DeployWizard.tsx`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`

- [ ] **Step 1: Add cloud account types and API methods**

In `api/types.ts`:
```typescript
export interface CloudAccountSummary {
  account_id: string;
  name: string;
  provider: 'azure' | 'aws' | 'gcp';
  region_default: string | null;
  personal: boolean;
  status: 'active' | 'disabled' | 'error';
  last_validated: string | null;
}

export interface CreateCloudAccountRequest {
  name: string;
  provider: 'azure' | 'aws' | 'gcp';
  credentials: Record<string, string>;
  region_default?: string;
  personal: boolean;
}
```

In `api/client.ts`:
```typescript
getCloudAccounts(projectId: string): Promise<CloudAccountSummary[]>
createCloudAccount(projectId: string, req: CreateCloudAccountRequest): Promise<{ account_id: string }>
updateCloudAccount(projectId: string, accountId: string, name: string, region?: string): Promise<void>
deleteCloudAccount(projectId: string, accountId: string): Promise<void>
validateCloudAccount(projectId: string, accountId: string): Promise<{ status: string; error?: string }>
```

- [ ] **Step 2: Create `CloudAccountsPage.tsx`**

Table layout:

```
┌──────────────────────────────────────────────────────────────┐
│  Cloud Accounts                              [+ Add Account] │
├──────────────────────────────────────────────────────────────┤
│  Production Azure    azure    eastus   shared   active  [...]│
│  Staging Azure       azure    westus   shared   active  [...]│
│  My Personal AWS     aws      us-east  personal active  [...]│
└──────────────────────────────────────────────────────────────┘
```

Features:
- "Add Account" button: opens a form with provider selector, name, credentials fields (per-provider: Azure needs tenant_id/client_id/client_secret, AWS needs access_key_id/secret_access_key, GCP needs JSON key paste), region, personal checkbox
- Credentials input fields use `type="password"` (masked)
- "Validate" button on each row: calls validate endpoint, shows spinner, updates status
- "Delete" button with confirmation
- Status badge: active=green, disabled=gray, error=red
- Personal accounts show "(personal)" label

Access: visible to Operator+. Only Admin can add shared accounts (personal checkbox hidden for non-admin adding shared). Only Admin can delete shared accounts.

- [ ] **Step 3: Create `CloudAccountSelector.tsx`**

Component for the deploy wizard:

```typescript
interface Props {
  projectId: string;
  provider: string;  // 'azure' | 'aws' | 'gcp'
  selectedAccountId: string | null;
  onSelect: (accountId: string) => void;
}
```

Fetches cloud accounts for the project filtered by provider. Shows a list of accounts with radio selection. Includes "+ Add new account" link that opens the account creation form inline.

- [ ] **Step 4: Update `DeployWizard.tsx`**

Insert a new Step 2 (after provider selection, before VM configuration):

```
Step 1: Select Provider  [Azure] [AWS] [GCP] [LAN]
Step 2: Select Account   (NEW — CloudAccountSelector)
Step 3: Configure VM     (existing step, was step 2)
Step 4: Deploy           (existing step, was step 3)
```

When LAN is selected, skip Step 2 (no cloud account needed).

Pass the `cloud_account_id` in the deployment creation request body.

When the deployment uses a cloud account:
- The dashboard backend decrypts credentials server-side
- Passes them to the deploy runner
- Frontend never sees raw credentials

- [ ] **Step 5: Build + test**

```bash
cd dashboard && npm run build && npm run lint
```

Test: add Azure/AWS/GCP accounts. Validate works. Deploy wizard shows account selector. LAN deploy skips account selection.

- [ ] **Step 6: Commit PR 6**

```bash
git checkout -b feat/v015-frontend-cloud-accounts
git add -A
git commit -m "feat(v0.15): cloud accounts page + deploy wizard integration

- CloudAccountsPage: CRUD for project cloud accounts
- Per-provider credential forms (Azure SP, AWS keys, GCP service account)
- Credential validation with status indicators
- CloudAccountSelector in deploy wizard (new Step 2)
- LAN deployments skip account selection
- Credentials never exposed to browser"
```

---

## Task 7: Share Links (Backend + Frontend) (PR 7)

**Branch:** `feat/v015-share-links`
**Depends on:** PR 3, PR 4

**Files:**
- Create: `crates/networker-dashboard/src/api/share_links.rs`
- Create: `crates/networker-dashboard/src/db/share_links.rs`
- Create: `dashboard/src/pages/ShareLinksPage.tsx`
- Create: `dashboard/src/pages/ShareViewPage.tsx`
- Create: `dashboard/src/components/ShareDialog.tsx`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`

- [ ] **Step 1: Create `db/share_links.rs`**

Add `pub mod share_links;` to `db/mod.rs`.

```rust
pub struct ShareLinkRow {
    pub link_id: Uuid,
    pub project_id: Uuid,
    pub token_hash: String,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub label: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
    pub access_count: i32,
    pub last_accessed: Option<DateTime<Utc>>,
    // Joined:
    pub created_by_email: String,
}

pub async fn create_link(
    client: &Client,
    project_id: Uuid,
    token_hash: &str,
    resource_type: &str,
    resource_id: Option<Uuid>,
    label: Option<&str>,
    expires_at: DateTime<Utc>,
    created_by: Uuid,
) -> Result<Uuid>

pub async fn list_links(client: &Client, project_id: Uuid) -> Result<Vec<ShareLinkRow>>

pub async fn resolve_link(client: &Client, token_hash: &str) -> Result<Option<ShareLinkRow>>
// WHERE token_hash = $1 AND revoked = FALSE AND expires_at > now()
// Also: UPDATE share_link SET access_count = access_count + 1, last_accessed = now()

pub async fn revoke_link(client: &Client, link_id: Uuid, project_id: Uuid) -> Result<()>

pub async fn delete_link(client: &Client, link_id: Uuid, project_id: Uuid) -> Result<()>

pub async fn extend_link(client: &Client, link_id: Uuid, new_expires: DateTime<Utc>) -> Result<()>
```

- [ ] **Step 2: Create `api/share_links.rs`**

Add `mod share_links;` to `api/mod.rs`.

Project-scoped endpoints:
```
GET    /share-links       — list share links (admin)
POST   /share-links       — create share link (admin)
PUT    /share-links/:lid  — extend/revoke (admin)
DELETE /share-links/:lid  — delete (admin)
```

Public endpoint (no auth):
```
GET    /share/:token      — resolve share link and return resource
```

**Create handler logic:**
1. Parse body: `{ resource_type, resource_id, label, expires_in_days }`
2. Validate: `expires_in_days <= state.share_max_days`
3. Generate 32 random bytes, encode as URL-safe base64 (43 chars)
4. Compute SHA-256 hash of the raw token
5. Store hash in DB (never store raw token)
6. Return: `{ link_id, url: "${share_base_url}/share/${raw_token}", expires_at, label }`
7. The raw token is returned ONLY in this response

**Resolve handler logic (public, no auth):**
1. Extract `:token` from URL path
2. Base64-decode → 32 bytes (validate length)
3. SHA-256 hash the raw token
4. Look up in DB: `WHERE token_hash = hash AND revoked = FALSE AND expires_at > now()`
5. If not found → 404
6. Increment `access_count`, update `last_accessed`
7. Fetch the resource data based on `resource_type`:
   - `"run"`: fetch run detail (same as GET /api/projects/:pid/runs/:rid)
   - `"job"`: fetch job detail
8. Return resource JSON + share metadata (created_by email, expires_at)

Defer `"dashboard"` share links until a real snapshot storage model exists. This v0.15 plan supports run/job shares only.

Register the public `/share/:token` route in the `public` section of `api/mod.rs` (no auth middleware).

- [ ] **Step 3: Create `ShareDialog.tsx`**

Modal dialog triggered from RunDetailPage or JobDetailPage "Share" button for project admins only:

```
┌──────────────────────────────────────┐
│  Share this report                   │
├──────────────────────────────────────┤
│  Label: [Q1 latency report        ] │
│  Expires in: [30] days               │
│                                      │
│          [Cancel]  [Create Link]     │
└──────────────────────────────────────┘
```

After creation, show the URL with a "Copy" button. Warning: "This link will not be shown again."

- [ ] **Step 4: Create `ShareLinksPage.tsx`**

Table of share links:

```
┌────────────────────────────────────────────────────────────────────┐
│  Share Links                                                       │
├────────────────────────────────────────────────────────────────────┤
│  Q1 latency report  run   ●active   30 views  Expires Apr 20  [...]│
│  Smoke test result  job   ●active    5 views  Expires May 15  [...]│
│  Old report         run   ●revoked   0 views  Expired         [...]│
└────────────────────────────────────────────────────────────────────┘
```

Actions per row: Revoke, Extend (+30 days), Delete. Status badges: active=green, revoked=red, expired=gray.

- [ ] **Step 5: Create `ShareViewPage.tsx`**

Standalone page at `/share/:token`. No sidebar, no auth required.

Layout:
```
┌─────────────────────────────────────────────────────┐
│  AletheDash — Shared Report                         │
│  ─────────────────────────────────────              │
│                                                     │
│  [Reuse RunDetailPage/JobDetailPage content here]   │
│  (read-only: no edit buttons, no navigation)        │
│                                                     │
│  ─────────────────────────────────────              │
│  Shared by alice@company.com · Expires Apr 20       │
│  Powered by Networker                               │
└─────────────────────────────────────────────────────┘
```

On mount: fetch `GET /share/:token`. If 404, show "Link expired or invalid". Wrap existing detail components in a read-only context (hide action buttons).

- [ ] **Step 6: Update `App.tsx`**

Add the share route in the public (non-authenticated) section:
```tsx
<Route path="/share/:token" element={<ShareViewPage />} />
```

Add share link routes in the project-scoped section:
```tsx
<Route path="/projects/:projectId/share-links" element={<ShareLinksPage />} />
```

- [ ] **Step 7: Build + test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
cd dashboard && npm run build && npm run lint
```

Test: admin creates share link, copy URL, open in incognito (no auth). Test expiration. Test revocation. Test non-admin cannot access share controls.

- [ ] **Step 8: Commit PR 7**

```bash
git checkout -b feat/v015-share-links
git add -A
git commit -m "feat(v0.15): share links — expiring public URLs for test results

- 32-byte random tokens, SHA-256 hashed in DB
- Public /share/:token endpoint (no auth required)
- Maximum 365-day expiration (configurable via DASHBOARD_SHARE_MAX_DAYS)
- ShareLinksPage: manage links (revoke, extend, delete)
- ShareDialog: create link from any run/job detail page
- ShareViewPage: standalone read-only view
- Raw token returned only at creation (not stored)"
```

---

## Task 8: Command Approval + SSE Notifications (PR 8)

**Branch:** `feat/v015-command-approvals`
**Depends on:** PR 3, PR 4

**Files:**
- Create: `crates/networker-dashboard/src/api/command_approvals.rs`
- Create: `crates/networker-dashboard/src/api/events.rs`
- Create: `crates/networker-dashboard/src/db/command_approvals.rs`
- Create: `dashboard/src/pages/CommandApprovalsPage.tsx`
- Create: `dashboard/src/components/NotificationBell.tsx`
- Create: `dashboard/src/components/ApprovalDrawer.tsx`
- Create: `dashboard/src/hooks/useSSE.ts`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `crates/networker-dashboard/src/config.rs`
- Modify: `crates/networker-dashboard/src/deploy/runner.rs`
- Modify: `dashboard/src/components/layout/Sidebar.tsx`

- [ ] **Step 1: Add SSE broadcast channels to AppState**

In `main.rs`, add:
```rust
pub approval_tx: broadcast::Sender<String>,   // JSON-encoded approval events
pub user_status_tx: broadcast::Sender<String>, // JSON-encoded user status events
```

Initialize with capacity 100:
```rust
let (approval_tx, _) = broadcast::channel(100);
let (user_status_tx, _) = broadcast::channel(100);
```

- [ ] **Step 2: Create `db/command_approvals.rs`**

Add `pub mod command_approvals;` to `db/mod.rs`.

```rust
pub struct ApprovalRow {
    pub approval_id: Uuid,
    pub project_id: Uuid,
    pub agent_id: Uuid,
    pub agent_name: String,  // joined from agent table
    pub command_type: String,
    pub command_detail: serde_json::Value,
    pub status: String,
    pub requested_by: Uuid,
    pub requested_by_email: String,  // joined
    pub decided_by: Option<Uuid>,
    pub decided_by_email: Option<String>,  // joined
    pub requested_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub reason: Option<String>,
}

pub async fn create_approval(
    client: &Client,
    project_id: Uuid,
    agent_id: Uuid,
    command_type: &str,
    command_detail: serde_json::Value,
    requested_by: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<Uuid>
// command_detail must be replayable without the original request still being alive.
// Include at minimum: cloud_account_id, provider, deployment_id (if any),
// target VM identifier, region/resource_group, and the exact command payload.

pub async fn list_pending(client: &Client, project_id: Uuid) -> Result<Vec<ApprovalRow>>
// WHERE project_id = $1 AND status = 'pending' AND expires_at > now()

pub async fn get_pending_count(client: &Client, project_id: Uuid) -> Result<i64>

pub async fn decide(
    client: &Client,
    approval_id: Uuid,
    project_id: Uuid,
    decided_by: Uuid,
    approved: bool,
    reason: Option<&str>,
) -> Result<()>
// UPDATE command_approval SET status = 'approved'/'denied', decided_by, decided_at, reason

pub async fn expire_stale(client: &Client) -> Result<u64>
// UPDATE command_approval SET status = 'expired' WHERE status = 'pending' AND expires_at <= now()
```

- [ ] **Step 3: Create `api/events.rs` — SSE endpoints**

```rust
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use tokio_stream::wrappers::BroadcastStream;

/// GET /api/events/approval — SSE stream (JWT required)
pub async fn approval_events(
    Extension(auth): Extension<AuthUser>,
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.approval_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(data) => Some(Ok(Event::default().event("approval").data(data))),
            Err(_) => None,  // lagged — skip
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// GET /api/events/user-status — SSE stream (JWT required)
pub async fn user_status_events(
    Extension(auth): Extension<AuthUser>,
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.user_status_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| {
        match result {
            Ok(data) => Some(Ok(Event::default().event("status_changed").data(data))),
            Err(_) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
```

Register SSE endpoints in the protected routes section of `api/mod.rs`:
```rust
.route("/api/events/approval", get(events::approval_events))
.route("/api/events/user-status", get(events::user_status_events))
```

Keep these endpoints behind `require_auth` and use standard Authorization headers. Do not add `?token=` fallback handling to the handlers.

- [ ] **Step 4: Create `api/command_approvals.rs`**

Project-scoped endpoints:
```
GET  /command-approvals           — list pending approvals (admin)
POST /command-approvals/:aid      — approve/deny (admin)
                                    body: { approved: bool, reason?: string }
```

The approve handler:
1. Extract `ProjectContext`, require Admin role
2. Call `db::command_approvals::decide()`
3. Load the updated approval row (including replayable `command_detail`)
4. Broadcast SSE event: `approval_tx.send(json!({"approval_id": ..., "status": "approved/denied", ...}))`
5. If approved: trigger the actual command execution using the stored approval payload

- [ ] **Step 5: Update `deploy/runner.rs` for approval flow**

Before executing destructive commands (`vm_run_command`, `vm_delete`, `vm_stop`):
1. Create a `command_approval` record (status='pending', expires_at=now+1h)
   - Persist a fully replayable execution payload in `command_detail`
2. Broadcast SSE event to project admins
3. Return the approval_id to the caller
4. The actual execution is deferred until the admin approves via the API

Add a function `execute_approved_command` that is called after approval:
```rust
pub async fn execute_approved_command(
    state: &AppState,
    client: &Client,
    approval: &ApprovalRow,
) -> anyhow::Result<()> {
    // Load cloud_account_id from approval.command_detail
    // Decrypt credentials on demand from cloud_account
    // Execute the command (az vm run-command, aws ssm, etc.)
}
```

- [ ] **Step 6: Add expired approval cleanup to scheduler**

In `scheduler.rs`, add an hourly task:
```rust
// Expire stale approvals
let expired = db::command_approvals::expire_stale(&client).await?;
if expired > 0 {
    tracing::info!("Expired {expired} stale command approvals");
}
```

- [ ] **Step 7: Create `useSSE.ts` hook**

```typescript
import { useEffect } from 'react';
import { useAuthStore } from '../stores/authStore';

interface SSEOptions {
  onApproval?: (data: any) => void;
  onStatusChange?: (data: any) => void;
}

async function connectSse(
  path: string,
  token: string,
  handlers: Record<string, (data: any) => void>,
  signal: AbortSignal,
) {
  const res = await fetch(path, {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: 'text/event-stream',
    },
    signal,
  });
  // Parse SSE frames (event:/data:) from res.body and dispatch to handlers
}

export function useSSE(options: SSEOptions) {
  const token = useAuthStore(s => s.token);

  useEffect(() => {
    if (!token) return;

    const approvalAbort = new AbortController();
    const statusAbort = new AbortController();

    void connectSse('/api/events/approval', token, {
      approval: (data) => options.onApproval?.(data),
    }, approvalAbort.signal);

    void connectSse('/api/events/user-status', token, {
      status_changed: (data) => options.onStatusChange?.(data),
    }, statusAbort.signal);

    return () => {
      approvalAbort.abort();
      statusAbort.abort();
    };
  }, [token]);
}
```

Use fetch-based SSE so Authorization headers continue to work with the existing `require_auth` middleware. Do not rely on query-parameter JWTs.

- [ ] **Step 8: Create `NotificationBell.tsx`**

Sidebar component showing pending approval count:

```typescript
interface Props {
  projectId: string;
}

export function NotificationBell({ projectId }: Props) {
  const [count, setCount] = useState(0);

  // Fetch initial count
  useEffect(() => {
    api.getPendingApprovalCount(projectId).then(setCount);
  }, [projectId]);

  // SSE updates
  useSSE({
    onApproval: () => {
      // Refetch count on any approval event
      api.getPendingApprovalCount(projectId).then(setCount);
    },
  });

  if (count === 0) return null;

  return (
    <span className="bg-red-500 text-white text-xs rounded-full px-1.5 py-0.5 ml-1">
      {count}
    </span>
  );
}
```

- [ ] **Step 9: Create `ApprovalDrawer.tsx`**

Slide-out panel (right side) listing pending approvals:

```
┌──────────────────────────────────┐
│  Pending Approvals               │
├──────────────────────────────────┤
│  Update endpoint on vm-eastus-01 │
│  Requested by alice@co.com       │
│  2 minutes ago                   │
│  [Approve] [Deny]               │
│  ────────────────────────────    │
│  Delete VM vm-westus-02          │
│  Requested by bob@co.com         │
│  15 minutes ago                  │
│  [Approve] [Deny]               │
└──────────────────────────────────┘
```

Deny button opens a text input for reason. Approve button sends immediately. Both call `POST /api/projects/:pid/command-approvals/:aid`.

- [ ] **Step 10: Create `CommandApprovalsPage.tsx`**

Full page view of all approvals (pending + history):

```
Tabs: [Pending (2)] [History]
```

Pending tab: same as drawer content but full-width with more detail (command preview JSON). History tab: table of past approvals with status, decided_by, reason.

- [ ] **Step 11: Update `Sidebar.tsx`**

Add the notification bell next to "Approvals" nav item:
```tsx
{ path: `/projects/${pid}/approvals`, label: 'Approvals', icon: '⊘', minRole: 'admin', badge: <NotificationBell projectId={pid} /> }
```

- [ ] **Step 12: Build + test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
cd dashboard && npm run build && npm run lint
```

Test: operator triggers destructive command → approval created → admin gets SSE notification → admin approves → command executes.

- [ ] **Step 13: Commit PR 8**

```bash
git checkout -b feat/v015-command-approvals
git add -A
git commit -m "feat(v0.15): command approval flow + SSE notifications

- command_approval table: pending/approved/denied/expired lifecycle
- SSE endpoints: /api/events/approval and /api/events/user-status
- Destructive commands (vm_run_command, vm_delete, vm_stop) require admin approval
- 1-hour expiration on pending approvals, hourly cleanup
- NotificationBell in sidebar with live count
- ApprovalDrawer for quick approve/deny
- CommandApprovalsPage for full history"
```

---

## Task 9: Test Visibility Rules (PR 9)

**Branch:** `feat/v015-visibility-rules`
**Depends on:** PR 3, PR 4

**Files:**
- Create: `crates/networker-dashboard/src/api/visibility.rs`
- Create: `crates/networker-dashboard/src/db/visibility.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/api/jobs.rs`
- Modify: `crates/networker-dashboard/src/api/schedules.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `crates/networker-dashboard/src/db/jobs.rs`
- Modify: `crates/networker-dashboard/src/db/schedules.rs`

- [ ] **Step 1: Create `db/visibility.rs`**

Add `pub mod visibility;` to `db/mod.rs`.

```rust
pub struct VisibilityRuleRow {
    pub rule_id: Uuid,
    pub project_id: Uuid,
    pub user_id: Option<Uuid>,
    pub resource_type: String,
    pub resource_id: Uuid,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
}

pub async fn list_rules(client: &Client, project_id: Uuid) -> Result<Vec<VisibilityRuleRow>>

pub async fn add_rule(
    client: &Client,
    project_id: Uuid,
    user_id: Option<Uuid>,
    resource_type: &str,
    resource_id: Uuid,
    created_by: Uuid,
) -> Result<Uuid>

pub async fn remove_rule(client: &Client, rule_id: Uuid, project_id: Uuid) -> Result<()>

/// Get the set of resource_ids visible to a specific viewer in a project.
/// Returns None if project visibility is "all" (no filtering needed).
/// Returns Some(set) if "explicit" — the set contains visible resource_ids.
pub async fn visible_resources(
    client: &Client,
    project_id: Uuid,
    user_id: Uuid,
    resource_type: &str,
) -> Result<Option<HashSet<Uuid>>>
```

The `visible_resources` function:
1. Read `project.settings` JSONB to check `test_visibility` setting
2. If `"all"` (default): return None (no filtering)
3. If `"explicit"`: query `test_visibility_rule` for rules where `(user_id IS NULL OR user_id = $2) AND resource_type = $3`
4. Return `Some(set_of_resource_ids)`

- [ ] **Step 2: Create `api/visibility.rs`**

Project-scoped endpoints (admin only):
```
GET    /visibility-rules       — list all rules
POST   /visibility-rules       — add rule { user_id?, resource_type, resource_id }
DELETE /visibility-rules/:rid  — remove rule
```

- [ ] **Step 3: Update `db/jobs.rs` for visibility filtering**

Add a `list_jobs_with_visibility` function (or update `list_jobs`):

```rust
pub async fn list_jobs(
    client: &Client,
    project_id: Uuid,
    visible_ids: Option<&HashSet<Uuid>>,  // None = show all, Some = filter
) -> Result<Vec<JobRow>> {
    let rows = if let Some(ids) = visible_ids {
        // Filter by definition_id IN visible set
        // (Viewer sees only jobs whose test_definition is in the visible set)
        client.query(
            "SELECT ... FROM job j
             JOIN test_definition td ON j.definition_id = td.definition_id
             WHERE j.project_id = $1 AND td.definition_id = ANY($2)
             ORDER BY j.created_at DESC",
            &[&project_id, &ids.iter().collect::<Vec<_>>()],
        ).await?
    } else {
        client.query(
            "SELECT ... FROM job WHERE project_id = $1 ORDER BY created_at DESC",
            &[&project_id],
        ).await?
    };
    ...
}
```

- [ ] **Step 4: Update `db/schedules.rs` for visibility filtering**

Same pattern as jobs: accept `visible_ids: Option<&HashSet<Uuid>>` and filter schedules by definition_id.

- [ ] **Step 5: Update `api/jobs.rs` and `api/schedules.rs` handlers**

In `list_jobs` handler:
```rust
async fn list_jobs(
    Extension(auth): Extension<AuthUser>,
    Extension(ctx): Extension<ProjectContext>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let client = state.db.get().await?;

    // Apply visibility filtering only for viewers
    let visible = if ctx.role == ProjectRole::Viewer {
        db::visibility::visible_resources(
            &client, ctx.project_id, auth.user_id, "test_definition"
        ).await?
    } else {
        None  // Operators and admins see everything
    };

    let jobs = db::jobs::list_jobs(&client, ctx.project_id, visible.as_ref()).await?;
    Json(jobs)
}
```

Same pattern for schedules.

- [ ] **Step 6: (Frontend) Add visibility controls to Tests page**

This is a small UI addition to the existing Tests/JobsPage:
- For each test definition row, project admins see a "Visibility" dropdown: "All viewers" / "Specific viewers"
- When "Specific viewers" is selected, show a multi-select of viewer members
- Save changes via POST/DELETE to `/visibility-rules`

This does NOT need its own page — it integrates into the existing test definition management.

- [ ] **Step 7: Build + test**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard --lib
cd dashboard && npm run build && npm run lint
```

Test: set project to "explicit" visibility. Add rules for specific viewers. Verify viewer sees only allowed tests. Verify operator sees all.

- [ ] **Step 8: Commit PR 9**

```bash
git checkout -b feat/v015-visibility-rules
git add -A
git commit -m "feat(v0.15): test visibility rules for viewer role

- Visibility rules: per-test-definition, per-viewer or all-viewers
- Project setting: 'all' (default) or 'explicit' test_visibility
- Viewer filtering applied to jobs and schedules list endpoints
- Operators and admins always see everything
- Admin UI on Tests page for managing visibility"
```

---

## Task 10: Hardening — NOT NULL Migration + Remove Compat + Version Bump (PR 10)

**Branch:** `feat/v015-hardening`
**Depends on:** PRs 1-9 stable (one release cycle of soak time)

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/auth/mod.rs`
- Modify: `crates/networker-dashboard/src/db/agents.rs`
- Modify: `crates/networker-dashboard/src/db/jobs.rs`
- Modify: `crates/networker-dashboard/src/db/runs.rs`
- Modify: `crates/networker-dashboard/src/db/schedules.rs`
- Modify: `crates/networker-dashboard/src/db/deployments.rs`
- Modify: `Cargo.toml` (workspace version)
- Modify: `CHANGELOG.md`
- Modify: `install.sh`
- Modify: `install.ps1`

- [ ] **Step 1: Write V011_NOT_NULL migration**

In `db/migrations.rs`, add:

```sql
-- ============================================================
-- V011: Enforce NOT NULL on project_id columns (after soak period)
-- ============================================================

-- Verify no NULL project_ids remain (safety check — migration will fail if any exist)
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM agent WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found agent rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM test_definition WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found test_definition rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM job WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found job rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM schedule WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found schedule rows with NULL project_id — run backfill first';
    END IF;
    IF EXISTS (SELECT 1 FROM deployment WHERE project_id IS NULL) THEN
        RAISE EXCEPTION 'Found deployment rows with NULL project_id — run backfill first';
    END IF;
END $$;

ALTER TABLE agent ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE test_definition ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE job ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE schedule ALTER COLUMN project_id SET NOT NULL;
ALTER TABLE deployment ALTER COLUMN project_id SET NOT NULL;
```

Register in `run()` as V011.

- [ ] **Step 2: Remove backward-compat flat alias routes**

In `api/mod.rs`, remove the temporary flat alias routes (`/api/agents`, `/api/jobs`, etc.) after the frontend and bookmarks have soaked on project-scoped URLs.

- [ ] **Step 3: Remove NULL coalescing in DB queries**

Review all DB query functions for any `COALESCE(project_id, '000...001')` or `WHERE project_id IS NULL OR project_id = $1` patterns. Replace with simple `WHERE project_id = $1` now that NOT NULL is enforced.

- [ ] **Step 4: Remove legacy flat route handlers from API modules**

If any API modules still expose the old `router()` function (kept during transition), remove them. Only `project_router()` should remain.

- [ ] **Step 5: Clean up frontend legacy redirects**

In `App.tsx`, remove the `<LegacyRedirect>` routes for `/tests`, `/tests/:jobId`, `/runs`, `/runs/:runId`, `/deploy`, `/deploy/:deploymentId`, `/schedules`, `/settings`. Replace with a catch-all redirect to `/projects`.

- [ ] **Step 6: Version bump to v0.15.0**

Update all 3 sync locations:
1. `Cargo.toml` workspace version: `"0.15.0"`
2. `CHANGELOG.md`: add `## [0.15.0]` section with full feature summary
3. `install.sh`: `INSTALLER_VERSION="v0.15.0"`
4. `install.ps1`: `$InstallerVersion = "v0.15.0"`

Run `cargo generate-lockfile` to update `Cargo.lock`.

- [ ] **Step 7: Build + full test suite**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --lib
cd dashboard && npm run build && npm run lint
```

- [ ] **Step 8: Commit PR 10**

```bash
git checkout -b feat/v015-hardening
git add -A
git commit -m "feat(v0.15.0): NOT NULL enforcement + remove flat compat + version bump

- V011 migration: enforce NOT NULL on project_id columns (with safety checks)
- Remove temporary flat compatibility routes (old flat /api/*)
- Remove NULL coalescing in DB queries
- Remove legacy frontend route redirects
- Version bump: 0.14.x → 0.15.0 (Cargo.toml, CHANGELOG, installers)"
```

---

## Version Strategy

PRs 1-9 are merged as v0.14.x point releases (e.g., v0.14.7 through v0.14.15). Each PR is independently deployable on top of the previous. The version bump to v0.15.0 is consolidated in PR 10 after all features are stable.

**Rationale:** This avoids a big-bang v0.15.0 that must ship all-or-nothing. If any feature is delayed, the others are already live. The version bump signals "multi-project is the default, old routes are gone."

---

## Post-Implementation Checklist

- [ ] All 10 PRs merged to main
- [ ] Existing v0.14 deployments migrate cleanly (V010 creates Default project, moves resources)
- [ ] Existing v0.14 roles are preserved when seeding Default project membership
- [ ] Tag `v0.15.0` and push (triggers release build)
- [ ] Verify release includes all 5 crate binaries
- [ ] Deploy to Azure VM via Settings → Update
- [ ] Test: existing single-project deployment works unchanged after migration
- [ ] Test: create second project, add members, verify isolation
- [ ] Test: URL-test endpoints are project-scoped and do not leak cross-project data
- [ ] Test: cloud account CRUD (Azure SP, AWS keys, GCP service account)
- [ ] Test: cloud account validation calls succeed
- [ ] Test: deploy VM using stored cloud credentials (not ambient CLI login)
- [ ] Test: share link creation, public access, expiration, revocation
- [ ] Test: command approval flow (operator request → admin SSE notification → approve → execute)
- [ ] Test: visibility rules (explicit mode, viewer sees only allowed tests)
- [ ] Test: project switcher in sidebar with multiple projects
- [ ] Test: platform admin can access all projects
- [ ] Test: old flat URL bookmarks and API calls still resolve correctly during the compatibility window (before PR 10 removes them)
- [ ] Update CLAUDE.md with new env vars (`DASHBOARD_CREDENTIAL_KEY`, `DASHBOARD_SHARE_URL`, `DASHBOARD_SHARE_MAX_DAYS`)
- [ ] Update Gist with new installer versions
- [ ] Run `shellcheck install.sh` and `bats tests/installer.bats`
