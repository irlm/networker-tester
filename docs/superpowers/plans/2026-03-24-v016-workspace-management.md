# v0.16 Workspace Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add workspace-level invite flow, system admin monitoring dashboard, and automated inactivity cleanup with soft-delete to AletheDash.

**Architecture:** V012 migration adds invite/warning tables + soft-delete columns. Invite flow uses SHA-256 hashed tokens (same pattern as share links). System metrics via `sysinfo` crate + `pg_stat_*`. Log buffer captures tracing events in a ring buffer. Scheduler runs daily inactivity checks alongside existing 30s schedule loop.

**Tech Stack:** Rust (axum 0.7, tokio-postgres, sysinfo 0.33), React 19, TypeScript, Zustand 5, Tailwind 4, Azure Communication Services

**Spec:** `docs/superpowers/specs/2026-03-24-v016-workspace-management-design.md`

---

## Task 1: V012 Migration + Invite Backend (PR 1)

**Branch:** `feat/v016-pr1-invite-backend`
**Depends on:** v0.15 (current main)

**Files:**
- Modify: `crates/networker-dashboard/src/db/migrations.rs`
- Create: `crates/networker-dashboard/src/db/invites.rs`
- Create: `crates/networker-dashboard/src/api/invites.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/db/projects.rs`
- Modify: `crates/networker-dashboard/src/auth/mod.rs`

- [ ] **Step 1: Add V012 migration**

In `db/migrations.rs`, add `V012_WORKSPACE_MANAGEMENT` SQL constant and register it in the `run()` function:

```sql
-- V012: Workspace management — invites, warnings, soft-delete

-- 1. Workspace invite table
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

-- 2. Workspace warning tracking (for inactivity emails)
CREATE TABLE IF NOT EXISTS workspace_warning (
    warning_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id   UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    warning_type VARCHAR(30) NOT NULL,
    sent_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_workspace_warning_unique ON workspace_warning (project_id, warning_type);

-- 3. Soft-delete columns on project
ALTER TABLE project ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;
ALTER TABLE project ADD COLUMN IF NOT EXISTS delete_protection BOOLEAN NOT NULL DEFAULT FALSE;

-- 4. Protect the Default workspace from auto-deletion
UPDATE project SET delete_protection = TRUE WHERE project_id = '00000000-0000-0000-0000-000000000001';
```

- [ ] **Step 2: Create `db/invites.rs`**

Add `pub mod invites;` to `db/mod.rs`.

Structs:
```rust
#[derive(Debug, Serialize)]
pub struct InviteRow {
    pub invite_id: Uuid,
    pub project_id: Uuid,
    pub email: String,
    pub role: String,
    pub status: String,
    pub invited_by: Uuid,
    pub invited_by_email: String,  // joined from dash_user
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
}
```

Functions:
- `create_invite(client, project_id, email, role, token_hash, invited_by, expires_at)` → Result<Uuid>
- `list_invites(client, project_id)` → Result<Vec<InviteRow>> — JOIN dash_user for invited_by_email, ORDER BY created_at DESC
- `resolve_invite(client, token_hash)` → Result<Option<InviteRow>> — WHERE token_hash = $1 AND status = 'pending' AND expires_at > now(). Also return project name (JOIN project).
- `accept_invite(client, invite_id, user_id)` → Result<()> — UPDATE status = 'accepted', accepted_at = now(), accepted_by = user_id
- `revoke_invite(client, invite_id, project_id)` → Result<()> — UPDATE status = 'revoked' WHERE invite_id = $1 AND project_id = $2 AND status = 'pending'
- `expire_stale_invites(client)` → Result<u64> — UPDATE status = 'expired' WHERE status = 'pending' AND expires_at <= now()

Include a `ResolvedInvite` struct for the resolve endpoint that includes project_name and whether the email already has an account:
```rust
#[derive(Debug, Serialize)]
pub struct ResolvedInvite {
    pub invite_id: Uuid,
    pub project_name: String,
    pub email: String,
    pub role: String,
    pub has_account: bool,
    pub expires_at: DateTime<Utc>,
}
```

- [ ] **Step 3: Create `api/invites.rs`**

Add `mod invites;` to `api/mod.rs`.

**Project-scoped endpoints (workspace admin):**

`GET /invites` — list invites for project. Extract ProjectContext, require Admin role. Call `db::invites::list_invites`.

`POST /invites` — create invite. Extract ProjectContext, require Admin role. Parse body `{ email: String, role: String }`. Validate role is one of admin/operator/viewer. Generate 32 random bytes, base64url-encode as token string. SHA-256 hash the token string. Compute `expires_at` from config (default 7 days, read `DASHBOARD_INVITE_EXPIRY_DAYS` from AppState). Call `db::invites::create_invite`. Send invite email via `crate::email::send_email`. Return `{ invite_id, url: "${public_url}/invite/${token}", expires_at }`.

`DELETE /invites/:invite_id` — revoke invite. Extract ProjectContext, require Admin. Parse invite_id from path. Call `db::invites::revoke_invite`.

Create `project_router()` function with these routes. Register in `api/mod.rs` project-scoped router.

**Public endpoints (no auth):**

`GET /invite/:token` — resolve invite. SHA-256 hash the token. Call `db::invites::resolve_invite`. If not found, return 404. Check if email has an existing account via `db::users::find_by_email`. Return `ResolvedInvite` JSON.

`POST /invite/:token/accept` — accept invite. Parse body: `{ password?: String }`. SHA-256 hash the token. Resolve the invite (must be pending + not expired). If user has an account: verify they're logged in (check Authorization header) OR if they provided password, authenticate them. If no account: require `password` in body (min 8 chars), create new user via `db::users::create_local_user` (new function). Call `db::invites::accept_invite`. Add user to project via `db::projects::add_member`. Issue JWT token. Return `{ token, email, role, project_id }`.

Register public routes in the `public` section of `api/mod.rs` (no auth middleware).

- [ ] **Step 4: Add `create_local_user` to `db/users.rs`**

```rust
pub async fn create_local_user(
    client: &Client,
    email: &str,
    password: &str,
) -> anyhow::Result<Uuid> {
    let user_id = Uuid::new_v4();
    let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow::anyhow!("{e}"))?;
    client.execute(
        "INSERT INTO dash_user (user_id, email, password_hash, role, status, auth_provider, must_change_password, is_platform_admin) \
         VALUES ($1, $2, $3, 'viewer', 'active', 'local', FALSE, FALSE)",
        &[&user_id, &email, &hash],
    ).await?;
    Ok(user_id)
}
```

- [ ] **Step 5: Add `invite_expiry_days` to config and AppState**

In `config.rs`, add:
```rust
pub invite_expiry_days: u32,
```
Parse from env: `DASHBOARD_INVITE_EXPIRY_DAYS`, default 7.

In `main.rs` AppState, add the field and initialize from config.

- [ ] **Step 6: Update `db/projects.rs` — add soft-delete filter**

Update `list_user_projects` to exclude soft-deleted workspaces:
- Add `WHERE p.deleted_at IS NULL` to both the admin and non-admin queries.

Update `get_project` to return `deleted_at` and `delete_protection` in ProjectRow.

- [ ] **Step 7: Update `auth/mod.rs` — check soft-delete in require_project**

In the `require_project` middleware, after fetching the project, check if `deleted_at IS NOT NULL`. If so, return 403 "Workspace suspended". Platform admins bypass this check (they need access for restore operations via admin API).

- [ ] **Step 8: Add invite expiry to scheduler**

In `scheduler.rs`, add `db::invites::expire_stale_invites` call in the scheduler loop. Run every cycle (30s) since it's a cheap UPDATE.

- [ ] **Step 9: Build + test**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --lib
```

- [ ] **Step 10: Commit PR 1**

---

## Task 2: Invite Frontend (PR 2)

**Branch:** `feat/v016-pr2-invite-frontend`
**Depends on:** PR 1

**Files:**
- Create: `dashboard/src/pages/AcceptInvitePage.tsx`
- Modify: `dashboard/src/pages/ProjectMembersPage.tsx`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`
- Modify: `dashboard/src/App.tsx`

- [ ] **Step 1: Add invite types and API methods**

In `api/types.ts`, add:
```typescript
export interface WorkspaceInvite {
  invite_id: string;
  project_id: string;
  email: string;
  role: string;
  status: string;
  invited_by: string;
  invited_by_email: string;
  created_at: string;
  expires_at: string;
}

export interface ResolvedInvite {
  invite_id: string;
  project_name: string;
  email: string;
  role: string;
  has_account: boolean;
  expires_at: string;
}
```

In `api/client.ts`, add:
```typescript
getInvites(projectId: string): Promise<WorkspaceInvite[]>
createInvite(projectId: string, email: string, role: string): Promise<{ invite_id: string; url: string; expires_at: string }>
revokeInvite(projectId: string, inviteId: string): Promise<void>
resolveInvite(token: string): Promise<ResolvedInvite>
acceptInvite(token: string, password?: string): Promise<{ token: string; email: string; role: string; project_id: string }>
```

Note: `resolveInvite` and `acceptInvite` call `/api/invite/:token` (public, no auth header needed for resolve; accept may need auth or password).

- [ ] **Step 2: Update `ProjectMembersPage.tsx`**

Replace "Add Member" button with "Invite" button. Add pending invites section above the members table:

- Fetch invites via `api.getInvites(projectId)` on mount
- Show pending invites in a simple list: email, role badge, expires in X days, "Revoke" button
- "Invite" button opens inline form: email input + role selector + "Send Invite" button
- On success: show toast with "Invite sent" + copy link button
- Keep the existing "Add existing user" as a small text link below the invite form for adding users who already have accounts without sending email

- [ ] **Step 3: Create `AcceptInvitePage.tsx`**

Standalone page at `/invite/:token`. No sidebar, no auth required (like ShareViewPage).

On mount: call `api.resolveInvite(token)`.
- If 404: show "Invitation expired or invalid"
- If found: show workspace name, role, and:
  - If `has_account`: "You already have an account. Log in to accept." + email/password form + SSO buttons
  - If new: "Create your account to join [workspace]" + email (pre-filled, read-only) + password + confirm password form
- On submit: call `api.acceptInvite(token, password?)`
- On success: store token in authStore, fetch projects, navigate to the workspace

Style: dark centered page like login, with "AletheDash" header, workspace name prominent.

- [ ] **Step 4: Update `App.tsx`**

Add `/invite/:token` route in the public section (before auth check):
```tsx
<Route path="/invite/:token" element={<AcceptInvitePage />} />
```

- [ ] **Step 5: Build + test**

```bash
cd dashboard && npm run build && npm run lint
```

- [ ] **Step 6: Commit PR 2**

---

## Task 3: System Metrics Backend (PR 3)

**Branch:** `feat/v016-pr3-system-metrics`
**Depends on:** PR 1

**Files:**
- Create: `crates/networker-dashboard/src/system_metrics.rs`
- Create: `crates/networker-dashboard/src/log_buffer.rs`
- Create: `crates/networker-dashboard/src/api/admin.rs`
- Modify: `crates/networker-dashboard/Cargo.toml`
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`

- [ ] **Step 1: Add `sysinfo` dependency**

In `crates/networker-dashboard/Cargo.toml`:
```toml
sysinfo = "0.33"
```

- [ ] **Step 2: Create `system_metrics.rs`**

Add `mod system_metrics;` to `main.rs`.

```rust
use serde::Serialize;
use sysinfo::System;
use tokio_postgres::Client;

#[derive(Debug, Serialize)]
pub struct SystemMetrics {
    pub cpu_usage_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct DbMetrics {
    pub active_connections: i64,
    pub max_connections: i64,
    pub database_size_bytes: i64,
    pub oldest_transaction_age_seconds: Option<f64>,
    pub cache_hit_ratio: f64,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceUsage {
    pub project_id: uuid::Uuid,
    pub name: String,
    pub slug: String,
    pub member_count: i64,
    pub tester_count: i64,
    pub jobs_30d: i64,
    pub runs_30d: i64,
    pub last_activity: Option<chrono::DateTime<chrono::Utc>>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub delete_protection: bool,
}

pub fn collect_system_metrics() -> SystemMetrics {
    let mut sys = System::new_all();
    sys.refresh_all();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let (disk_used, disk_total) = disks.iter().next().map(|d| (d.total_space() - d.available_space(), d.total_space())).unwrap_or((0, 0));
    SystemMetrics {
        cpu_usage_percent: sys.global_cpu_usage(),
        memory_used_bytes: sys.used_memory(),
        memory_total_bytes: sys.total_memory(),
        disk_used_bytes: disk_used,
        disk_total_bytes: disk_total,
        uptime_seconds: System::uptime(),
    }
}

pub async fn collect_db_metrics(client: &Client) -> anyhow::Result<DbMetrics> {
    // Active connections
    let active: i64 = client.query_one("SELECT count(*) FROM pg_stat_activity WHERE state = 'active'", &[]).await?.get(0);
    // Max connections
    let max_str: String = client.query_one("SHOW max_connections", &[]).await?.get(0);
    let max: i64 = max_str.parse().unwrap_or(100);
    // Database size
    let db_size: i64 = client.query_one("SELECT pg_database_size(current_database())", &[]).await?.get(0);
    // Oldest transaction
    let oldest: Option<f64> = client.query_opt(
        "SELECT EXTRACT(EPOCH FROM (now() - xact_start)) FROM pg_stat_activity WHERE state != 'idle' AND xact_start IS NOT NULL ORDER BY xact_start ASC LIMIT 1",
        &[],
    ).await?.map(|r| r.get(0));
    // Cache hit ratio
    let ratio: f64 = client.query_one(
        "SELECT COALESCE(sum(blks_hit)::float / NULLIF(sum(blks_hit) + sum(blks_read), 0), 0) FROM pg_stat_database WHERE datname = current_database()",
        &[],
    ).await?.get(0);

    Ok(DbMetrics { active_connections: active, max_connections: max, database_size_bytes: db_size, oldest_transaction_age_seconds: oldest, cache_hit_ratio: ratio })
}

pub async fn collect_workspace_usage(client: &Client) -> anyhow::Result<Vec<WorkspaceUsage>> {
    let rows = client.query(
        "SELECT p.project_id, p.name, p.slug, p.deleted_at, p.delete_protection,
                (SELECT COUNT(*) FROM project_member pm WHERE pm.project_id = p.project_id) as member_count,
                (SELECT COUNT(*) FROM agent a WHERE a.project_id = p.project_id) as tester_count,
                (SELECT COUNT(*) FROM job j WHERE j.project_id = p.project_id AND j.created_at > now() - interval '30 days') as jobs_30d,
                (SELECT COUNT(*) FROM job j2 WHERE j2.project_id = p.project_id AND j2.run_id IS NOT NULL AND j2.created_at > now() - interval '30 days') as runs_30d,
                (SELECT MAX(u.last_login_at) FROM project_member pm2 JOIN dash_user u ON u.user_id = pm2.user_id WHERE pm2.project_id = p.project_id) as last_activity
         FROM project p ORDER BY p.name",
        &[],
    ).await?;
    Ok(rows.iter().map(|r| WorkspaceUsage {
        project_id: r.get("project_id"),
        name: r.get("name"),
        slug: r.get("slug"),
        member_count: r.get("member_count"),
        tester_count: r.get("tester_count"),
        jobs_30d: r.get("jobs_30d"),
        runs_30d: r.get("runs_30d"),
        last_activity: r.get("last_activity"),
        deleted_at: r.get("deleted_at"),
        delete_protection: r.get("delete_protection"),
    }).collect())
}
```

- [ ] **Step 3: Create `log_buffer.rs`**

Add `mod log_buffer;` to `main.rs`.

```rust
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub target: String,
    pub message: String,
}

pub struct LogBuffer {
    entries: Mutex<VecDeque<LogEntry>>,
    capacity: usize,
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self { entries: Mutex::new(VecDeque::with_capacity(capacity)), capacity })
    }

    pub fn push(&self, entry: LogEntry) {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= self.capacity { entries.pop_front(); }
        entries.push_back(entry);
    }

    pub fn recent(&self, count: usize, level_filter: Option<&str>, search: Option<&str>) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap();
        entries.iter().rev()
            .filter(|e| level_filter.map(|l| e.level.eq_ignore_ascii_case(l)).unwrap_or(true))
            .filter(|e| search.map(|s| e.message.to_lowercase().contains(&s.to_lowercase()) || e.target.to_lowercase().contains(&s.to_lowercase())).unwrap_or(true))
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter().rev().collect()
    }
}
```

Create a custom tracing layer `LogBufferLayer` that captures events:
```rust
use tracing_subscriber::Layer;

pub struct LogBufferLayer {
    buffer: Arc<LogBuffer>,
}

impl LogBufferLayer {
    pub fn new(buffer: Arc<LogBuffer>) -> Self { Self { buffer } }
}

impl<S: tracing::Subscriber> Layer<S> for LogBufferLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        self.buffer.push(LogEntry {
            timestamp: Utc::now(),
            level: event.metadata().level().to_string(),
            target: event.metadata().target().to_string(),
            message: visitor.0,
        });
    }
}

struct MessageVisitor(String);
impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" { self.0 = format!("{:?}", value); }
        else if !self.0.is_empty() { self.0.push_str(&format!(" {}={:?}", field.name(), value)); }
        else { self.0 = format!("{}={:?}", field.name(), value); }
    }
}
```

- [ ] **Step 4: Update `main.rs` — integrate log buffer + add to AppState**

Add `log_buffer: Arc<log_buffer::LogBuffer>` to `AppState`.

In `main()`, create the buffer before the tracing subscriber and add the layer:
```rust
let log_buf = log_buffer::LogBuffer::new(1000);
tracing_subscriber::fmt()
    .with_env_filter(...)
    .finish()
    .with(log_buffer::LogBufferLayer::new(log_buf.clone()))
    .init();
```

Note: this replaces the simple `tracing_subscriber::fmt().init()` with a layered subscriber. Use `tracing_subscriber::registry()` with both the fmt layer and the LogBufferLayer.

- [ ] **Step 5: Create `api/admin.rs`**

Add `mod admin;` to `api/mod.rs`.

System admin only endpoints. Each handler checks `auth_user.is_platform_admin`.

`GET /system/metrics` — call `system_metrics::collect_system_metrics()` and `system_metrics::collect_db_metrics()`. Return combined JSON.

`GET /system/usage` — call `system_metrics::collect_workspace_usage()`. Return JSON array.

`GET /system/logs` — parse query params: `level`, `search`, `limit` (default 200). Call `state.log_buffer.recent()`. Return JSON array.

`POST /workspaces/:pid/suspend` — Set `project.deleted_at = now()`. Return success.

`POST /workspaces/:pid/restore` — Set `project.deleted_at = NULL`, clear warnings. Return success.

`POST /workspaces/:pid/protect` — Toggle `project.delete_protection`. Return new value.

`DELETE /workspaces/:pid` — Hard delete. Require workspace to be suspended (`deleted_at IS NOT NULL`). Delete all associated data in cascade order per the spec. Return success.

Create a `router()` function that nests all routes under `/admin`. Register in `api/mod.rs` protected_flat section (with require_auth).

- [ ] **Step 6: Add workspace lifecycle DB functions to `db/projects.rs`**

```rust
pub async fn suspend_project(client: &Client, project_id: &Uuid) -> anyhow::Result<()>
pub async fn restore_project(client: &Client, project_id: &Uuid) -> anyhow::Result<()>
pub async fn toggle_protection(client: &Client, project_id: &Uuid) -> anyhow::Result<bool>  // returns new value
pub async fn hard_delete_project(client: &Client, project_id: &Uuid) -> anyhow::Result<()>  // cascade delete all
pub async fn find_inactive_workspaces(client: &Client, days: i64) -> anyhow::Result<Vec<ProjectRow>>
pub async fn find_suspended_for_hard_delete(client: &Client, retention_days: i64) -> anyhow::Result<Vec<ProjectRow>>
```

- [ ] **Step 7: Build + test**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --lib
```

- [ ] **Step 8: Commit PR 3**

---

## Task 4: System Admin Dashboard Frontend (PR 4)

**Branch:** `feat/v016-pr4-admin-dashboard`
**Depends on:** PR 3

**Files:**
- Create: `dashboard/src/pages/SystemDashboardPage.tsx`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/components/layout/Sidebar.tsx`

- [ ] **Step 1: Add types and API methods**

In `api/types.ts`:
```typescript
export interface SystemMetrics {
  cpu_usage_percent: number;
  memory_used_bytes: number;
  memory_total_bytes: number;
  disk_used_bytes: number;
  disk_total_bytes: number;
  uptime_seconds: number;
}

export interface DbMetrics {
  active_connections: number;
  max_connections: number;
  database_size_bytes: number;
  oldest_transaction_age_seconds: number | null;
  cache_hit_ratio: number;
}

export interface WorkspaceUsage {
  project_id: string;
  name: string;
  slug: string;
  member_count: number;
  tester_count: number;
  jobs_30d: number;
  runs_30d: number;
  last_activity: string | null;
  deleted_at: string | null;
  delete_protection: boolean;
}

export interface LogEntry {
  timestamp: string;
  level: string;
  target: string;
  message: string;
}
```

In `api/client.ts`:
```typescript
getSystemMetrics(): fetch('/api/admin/system/metrics')
getWorkspaceUsage(): fetch('/api/admin/system/usage')
getSystemLogs(params?: { level?: string; search?: string; limit?: number }): fetch('/api/admin/system/logs?...')
suspendWorkspace(projectId: string): POST '/api/admin/workspaces/:pid/suspend'
restoreWorkspace(projectId: string): POST '/api/admin/workspaces/:pid/restore'
protectWorkspace(projectId: string): POST '/api/admin/workspaces/:pid/protect'
hardDeleteWorkspace(projectId: string): DELETE '/api/admin/workspaces/:pid'
```

- [ ] **Step 2: Create `SystemDashboardPage.tsx`**

Three tabs: Overview, Workspace Usage, System Logs.

**Overview tab:**
- Server section: CPU gauge (percentage + bar), Memory bar (used/total), Disk bar (used/total), Uptime
- Database section: Active/max connections, DB size (format as MB/GB), Cache hit ratio (percentage), Oldest transaction
- Application section: Dashboard version (from existing version API), total users count, total workspaces count
- Auto-refresh every 30s via usePolling

**Workspace Usage tab:**
- Table with columns from WorkspaceUsage type
- Status derived from: `deleted_at` not null → "suspended", `last_activity` > 60 days → "at-risk", > 90 days → "warning", else "active"
- Color coding: at-risk = yellow text, suspended = strikethrough + gray
- Actions column: Suspend button (for active), Restore button (for suspended), Protect toggle (shield icon), Hard Delete (red, only for suspended, with confirm)

**System Logs tab:**
- Level filter dropdown: All, INFO, WARN, ERROR
- Text search input
- Log entries in a monospace scrollable container, auto-scroll to bottom
- Each entry: `[timestamp] [LEVEL] target — message`
- Color code levels: INFO=gray, WARN=yellow, ERROR=red
- Pause/resume auto-scroll toggle

Format helpers:
```typescript
function formatBytes(bytes: number): string — "1.2 GB", "456 MB"
function formatUptime(seconds: number): string — "3d 12h 45m"
function daysAgo(iso: string | null): string — "5 days ago", "never"
```

- [ ] **Step 3: Update Sidebar**

Add "System" nav item in the PLATFORM section, before Users, only for system admins:
```typescript
if (isPlatformAdmin) {
  platformItems.push({ path: '/admin/system', label: 'System', icon: '\u2318' });
}
```

- [ ] **Step 4: Update App.tsx**

Add route:
```tsx
<Route path="/admin/system" element={<SystemDashboardPage />} />
```

- [ ] **Step 5: Build + test**

```bash
cd dashboard && npm run build && npm run lint
```

- [ ] **Step 6: Commit PR 4**

---

## Task 5: Soft Delete + Inactivity Scheduler (PR 5)

**Branch:** `feat/v016-pr5-inactivity`
**Depends on:** PR 3

**Files:**
- Create: `crates/networker-dashboard/src/db/workspace_warnings.rs`
- Modify: `crates/networker-dashboard/src/scheduler.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `crates/networker-dashboard/src/db/projects.rs`

- [ ] **Step 1: Create `db/workspace_warnings.rs`**

Add `pub mod workspace_warnings;` to `db/mod.rs`.

```rust
pub async fn has_warning(client: &Client, project_id: &Uuid, warning_type: &str) -> anyhow::Result<bool>
pub async fn record_warning(client: &Client, project_id: &Uuid, warning_type: &str) -> anyhow::Result<()>
pub async fn clear_warnings(client: &Client, project_id: &Uuid) -> anyhow::Result<()>
```

- [ ] **Step 2: Update scheduler with daily inactivity check**

In `scheduler.rs`, add a `last_inactivity_check: Option<Instant>` that tracks when the daily check last ran. In the main loop (every 30s), if last check was > 24 hours ago (or never), run the inactivity check:

```rust
async fn check_workspace_inactivity(state: &AppState) {
    let client = match state.db.get().await { Ok(c) => c, Err(_) => return };

    // 1. Expire stale invites
    let _ = db::invites::expire_stale_invites(&client).await;

    // 2. Find inactive workspaces (90 days, not protected, not suspended)
    let inactive = match db::projects::find_inactive_workspaces(&client, 90).await {
        Ok(v) => v, Err(_) => return
    };
    for ws in &inactive {
        if db::workspace_warnings::has_warning(&client, &ws.project_id, "inactivity_90d").await.unwrap_or(true) {
            continue;
        }
        // Send warning email to all members
        // ... fetch members, send email to each ...
        let _ = db::workspace_warnings::record_warning(&client, &ws.project_id, "inactivity_90d").await;
        tracing::info!(workspace = %ws.name, "Sent 90-day inactivity warning");
    }

    // 3. Find workspaces warned 30+ days ago still inactive → suspend
    // Query: projects with inactivity_90d warning sent > 30 days ago AND still no recent activity AND not suspended
    // For each: set deleted_at = now()

    // 4. Find suspended workspaces approaching 1 year → warn system admin
    // Query: projects where deleted_at < now() - 360 days AND no hard_delete_5d warning sent
    // Send email to system admin

    // 5. Find suspended workspaces past 1 year → hard delete
    // Query: projects where deleted_at < now() - 365 days
    // For each: hard_delete_project

    // 6. Expire stale command approvals (existing)
    let _ = db::command_approvals::expire_stale(&client).await;
}
```

- [ ] **Step 3: Add email sending for inactivity warnings**

In the scheduler inactivity check, for each workspace getting a warning:
1. Fetch all members via `db::projects::list_members`
2. For each member, send email via `crate::email::send_email`
3. Use plain text template per the spec

- [ ] **Step 4: Build + test**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --lib
```

- [ ] **Step 5: Commit PR 5**

---

## Task 6: Email Custom Domain Setup (PR 6)

**Branch:** `feat/v016-pr6-email-domain`
**Depends on:** None (infrastructure task)

This is a manual infrastructure task, not a code PR. Document the steps:

- [ ] **Step 1: Add custom domain in Azure portal**

1. Go to Azure Communication Services → alethedash-comm → Email → Domains
2. Add custom domain: `alethedash.com`
3. Note the DNS records Azure provides (SPF, DKIM CNAME records, DMARC)

- [ ] **Step 2: Add DNS records**

Add to `alethedash.com` DNS:
- SPF TXT record
- DKIM CNAME records (2 records from ACS)
- DMARC TXT record: `v=DMARC1; p=reject;`

- [ ] **Step 3: Verify domain in ACS**

Wait for DNS propagation, verify in Azure portal.

- [ ] **Step 4: Update sender address**

On the live server:
```bash
# In /etc/networker-dashboard.env:
DASHBOARD_ACS_SENDER=noreply@alethedash.com
```
Restart the dashboard service.

- [ ] **Step 5: Test email delivery**

Send a forgot-password email and verify it lands in inbox (not junk).

---

## Task 7: Hardening + Version Bump (PR 7)

**Branch:** `feat/v016-pr7-hardening`
**Depends on:** PRs 1-5

**Files:**
- Modify: `Cargo.toml` (workspace version → 0.16.0)
- Modify: `CHANGELOG.md`
- Modify: `install.sh` (INSTALLER_VERSION)
- Modify: `install.ps1` ($InstallerVersion)

- [ ] **Step 1: Version bump**

1. `Cargo.toml`: version = "0.16.0"
2. `CHANGELOG.md`: Add `## [0.16.0]` section
3. `install.sh`: `INSTALLER_VERSION="v0.16.0"`
4. `install.ps1`: `$InstallerVersion = "v0.16.0"`

- [ ] **Step 2: Run `cargo generate-lockfile`**

- [ ] **Step 3: Full test suite**

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --lib
cd dashboard && npm run build && npm run lint
bash tests/v015_api_tests.sh  # existing tests still pass
```

- [ ] **Step 4: Commit PR 7**
