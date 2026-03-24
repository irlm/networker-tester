# v0.16 Workspace Management Design Spec

## Goal

Extend AletheDash with workspace-level user management, self-service onboarding via invite links, a system admin monitoring dashboard, and automated inactivity cleanup with soft-delete — enabling multi-tenant operation where workspace admins independently manage their teams while system admins maintain oversight of the entire platform.

## Architecture

Three-level role hierarchy (System Admin > Workspace Admin > Operator > Viewer). Workspace admins invite members via email links. System admins monitor server health, workspace usage, and manage the lifecycle of workspaces including soft-delete and permanent cleanup. Email delivery via Azure Communication Services with custom domain for inbox deliverability.

## Tech Stack

- Backend: Rust (axum 0.7, tokio-postgres, sysinfo crate)
- Frontend: React 19, TypeScript, Zustand 5, Tailwind 4
- Email: Azure Communication Services (ACS)
- Metrics: sysinfo crate (CPU/memory/disk) + pg_stat_* (DB metrics)
- Scheduler: Existing tokio background task (30s interval)

---

## 1. Role Model & Workspace Membership

### Roles

| Level | Role | Scope | Capabilities |
|---|---|---|---|
| System | System Admin | Global | Create workspaces, view system metrics/logs, manage all users, see usage per workspace, configure inactivity policies, soft-delete/restore workspaces |
| Workspace | Admin | Per-workspace | Manage workspace members (invite/approve/remove), configure cloud accounts, manage approvals, share links, all operator capabilities |
| Workspace | Operator | Per-workspace | Run tests, create schedules, deploy endpoints, manage testers |
| Workspace | Viewer | Per-workspace | View test results, runs, dashboards (read-only) |

### Rules

- A user can belong to multiple workspaces with a different role in each
- System admin has implicit admin access to all workspaces (existing `is_platform_admin` flag)
- Workspace membership stored in existing `project_member` table
- Only system admin can create or delete workspaces
- Workspace admin manages members within their workspace independently

### What exists (v0.15)

The role model is already implemented. `ProjectRole` enum (Viewer/Operator/Admin) with `has_permission` ordering. `require_project` middleware checks membership. `is_platform_admin` on `dash_user` identifies system admins. `project_member` table stores per-workspace roles. No structural changes needed for the role model itself.

---

## 2. User Onboarding & Invite Flow

### Path 1: Invite Link (primary onboarding method)

1. Workspace admin clicks "Invite" on the Members page, enters email + role
2. System generates a 32-byte random invite token, stores SHA-256 hash in DB
3. ACS sends email: "You've been invited to join [Workspace Name] on AletheDash"
4. Recipient clicks link, lands on `/invite/:token` page
5. If they already have an account: log in, auto-join the workspace with the specified role
6. If new user: create account (email + password form), auto-join the workspace
7. No pending approval needed — the invite IS the approval

### Path 2: SSO Sign-up (existing, unchanged)

1. User clicks "Continue with Microsoft/Google" on login page
2. SSO creates account with `status = 'pending'`
3. System admin approves the user
4. After approval, workspace admin invites them to a workspace via Path 1

### Invite Link Details

- 32-byte random token, SHA-256 hashed in DB (same pattern as share links)
- Expires in 7 days (configurable via `DASHBOARD_INVITE_EXPIRY_DAYS`, default 7)
- One-time use: consumed when accepted
- Can be revoked by workspace admin before use
- Workspace admin can re-invite the same email (generates new token)

### Email Content

Subject: `You've been invited to [Workspace Name] — AletheDash`

Body:
```
Hi,

You've been invited to join the workspace "[Workspace Name]" on AletheDash
as a [role].

Click the link below to accept the invitation:

https://alethedash.com/invite/{token}

This invitation expires in 7 days.

If you don't have an AletheDash account, you'll be able to create one
when you accept.

— AletheDash
```

### Database: `workspace_invite` table

```sql
CREATE TABLE workspace_invite (
    invite_id       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id      UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    email           VARCHAR(255) NOT NULL,
    role            VARCHAR(20) NOT NULL DEFAULT 'viewer',
    token_hash      VARCHAR(128) NOT NULL,
    status          VARCHAR(20) NOT NULL DEFAULT 'pending',  -- pending, accepted, revoked, expired
    invited_by      UUID NOT NULL REFERENCES dash_user(user_id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at      TIMESTAMPTZ NOT NULL,
    accepted_at     TIMESTAMPTZ,
    accepted_by     UUID REFERENCES dash_user(user_id)
);
CREATE INDEX ix_workspace_invite_token ON workspace_invite (token_hash);
CREATE INDEX ix_workspace_invite_project ON workspace_invite (project_id, status);
```

### API Endpoints

**Project-scoped (workspace admin):**
```
GET    /api/projects/:pid/invites          — list pending/recent invites
POST   /api/projects/:pid/invites          — create invite { email, role }
DELETE /api/projects/:pid/invites/:iid     — revoke invite
```

**Public (no auth):**
```
GET    /api/invite/:token                  — resolve invite (returns workspace name, role, whether user exists)
POST   /api/invite/:token/accept           — accept invite { password? } (creates account if needed, joins workspace)
```

### Frontend

- Members page: "Invite" button opens form (email + role selector)
- After invite: show success with option to copy invite link
- New page: `/invite/:token` — standalone page (no sidebar), shows workspace name and role, form to accept (login or create account)
- Members page shows pending invites section below the members table

---

## 3. Workspace Admin Member Management

### What workspace admins can do (independently from system admin)

- View all members of their workspace
- Invite new members via email (creates invite link)
- Change member roles within the workspace
- Remove members from the workspace
- View and revoke pending invites
- Cannot remove themselves (prevents orphaned workspaces)
- Cannot change the last admin's role (existing v0.15 guard)

### What workspace admins cannot do

- Create or delete workspaces
- See members of other workspaces
- Access system admin dashboard
- Approve SSO sign-ups (that's system admin level)
- Modify `is_platform_admin` flag

### UI: Enhanced Members Page

The existing `ProjectMembersPage` gets these additions:

1. **Pending Invites section** — above the members table, shows pending invites with email, role, invited by, expires in, revoke button
2. **Invite button** — replaces "Add Member" with "Invite" (sends email instead of requiring user to already exist)
3. **"Add existing user" option** — for adding users who already have accounts (current behavior, kept as secondary option)

---

## 4. System Admin Dashboard

### New page: `/admin/system` (system admin only, not workspace-scoped)

Added to the sidebar under the PLATFORM section for system admins only.

### Tab 1: Overview

- **Server metrics** (via `sysinfo` crate):
  - CPU usage % (current + 1-min average)
  - Memory: used / total (GB) + percentage
  - Disk: used / total (GB) + percentage for the data partition
  - System uptime
- **Database metrics** (via PostgreSQL `pg_stat_*`):
  - Active connections / max connections
  - Database size (human-readable)
  - Oldest active transaction age
  - Cache hit ratio (from `pg_stat_database`)
- **Application info**:
  - Dashboard version
  - Uptime (process start time)
  - Total users, total workspaces
- Auto-refresh every 30s

### Tab 2: Workspace Usage

Table with columns:
- Workspace name
- Members count
- Testers count
- Jobs (30 days)
- Runs (30 days)
- Last activity (most recent login by any member)
- Status: active / at-risk (>60 days) / warning-sent (>90 days) / suspended

Sortable by any column. Rows with >60 days inactivity highlighted yellow. Suspended workspaces shown with strikethrough.

Actions per row:
- "Suspend" (soft-delete)
- "Protect" (toggle delete protection)
- "Delete permanently" (with confirmation, only for suspended workspaces)

### Tab 3: System Logs

- Recent dashboard log entries from an in-memory ring buffer (last 1000 entries)
- Filter by level: ALL, INFO, WARN, ERROR
- Text search filter
- Auto-scroll with pause button
- Entries show: timestamp, level, target module, message
- Not persistent — clears on restart. For persistent logs, use `journalctl`

### Backend Implementation

**New module: `crates/networker-dashboard/src/system_metrics.rs`**

```rust
pub struct SystemMetrics {
    pub cpu_usage_percent: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    pub uptime_seconds: u64,
}

pub struct DbMetrics {
    pub active_connections: i64,
    pub max_connections: i64,
    pub database_size_bytes: i64,
    pub oldest_transaction_age_seconds: Option<i64>,
    pub cache_hit_ratio: f64,
}

pub struct WorkspaceUsage {
    pub project_id: Uuid,
    pub name: String,
    pub member_count: i64,
    pub tester_count: i64,
    pub jobs_30d: i64,
    pub runs_30d: i64,
    pub last_activity: Option<DateTime<Utc>>,
    pub status: String,
    pub delete_protection: bool,
}

pub fn collect_system_metrics() -> SystemMetrics { ... }
pub async fn collect_db_metrics(client: &Client) -> DbMetrics { ... }
pub async fn collect_workspace_usage(client: &Client) -> Vec<WorkspaceUsage> { ... }
```

**New module: `crates/networker-dashboard/src/log_buffer.rs`**

```rust
use std::collections::VecDeque;
use std::sync::Mutex;

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
    pub fn new(capacity: usize) -> Self { ... }
    pub fn push(&self, entry: LogEntry) { ... }
    pub fn recent(&self, count: usize, level_filter: Option<&str>, search: Option<&str>) -> Vec<LogEntry> { ... }
}
```

Integrated via a custom `tracing` layer that captures events into the `LogBuffer`.

**New API endpoints** (system admin only, under `/api/admin`):

```
GET /api/admin/system/metrics     — server + DB metrics
GET /api/admin/system/usage       — workspace usage table
GET /api/admin/system/logs        — filtered log entries { level?, search?, limit? }
```

**Cargo.toml addition:**
```toml
sysinfo = "0.33"
```

---

## 5. Automated Inactivity Cleanup (Soft Delete)

### What counts as inactive

No user in the workspace (any role) has `last_login_at` within the past 90 days.

### Lifecycle Timeline

```
Day 0-89:    Normal usage
Day 90:      WARNING — email sent to all workspace members
             "Your workspace [Name] will be suspended in 30 days due to inactivity"
Day 120:     SOFT DELETE — workspace suspended
             - All members lose access (API returns 403 "Workspace suspended")
             - Data preserved but not visible in UI
             - Workspace disappears from project switcher
             - System admin can see it in "Suspended" tab
Day 120-485: Data retained (1 year from soft delete)
Day 480:     Email → system admin: "[Name] will be permanently deleted in 5 days"
Day 485:     HARD DELETE — all data permanently removed
```

### Database Changes

Add columns to `project` table:
```sql
ALTER TABLE project ADD COLUMN deleted_at TIMESTAMPTZ;
ALTER TABLE project ADD COLUMN delete_protection BOOLEAN NOT NULL DEFAULT FALSE;
```

Add tracking table for sent warnings:
```sql
CREATE TABLE workspace_warning (
    warning_id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id   UUID NOT NULL REFERENCES project(project_id) ON DELETE CASCADE,
    warning_type VARCHAR(30) NOT NULL,  -- 'inactivity_90d', 'inactivity_reminder', 'hard_delete_5d'
    sent_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(project_id, warning_type)
);
```

### Soft Delete Behavior

- When `project.deleted_at IS NOT NULL`:
  - `require_project` middleware returns 403 "Workspace suspended due to inactivity. Contact your system admin."
  - `list_user_projects` excludes soft-deleted workspaces (user never sees them)
  - System admin's workspace usage table shows them in "Suspended" tab
  - All data (agents, jobs, runs, etc.) remains in the DB untouched

### System Admin Controls

- **Suspend manually**: Set `deleted_at = now()` on any workspace at any time
- **Restore**: Set `deleted_at = NULL` — members regain access immediately, warnings cleared
- **Protect**: Set `delete_protection = true` — workspace is never auto-suspended or auto-deleted. Default workspace (UUID 000...001) always has protection enabled.
- **Hard delete**: Permanently remove workspace + all associated data (CASCADE). Only available for suspended workspaces. Requires confirmation.

### Hard Delete Cascade

When permanently deleting a workspace, delete in order:
1. `workspace_warning` (FK cascade)
2. `workspace_invite` (FK cascade)
3. `test_visibility_rule` (FK cascade)
4. `command_approval`
5. `share_link`
6. `cloud_account`
7. `schedule`
8. `job` (and associated `TestRun` data)
9. `deployment`
10. `agent`
11. `project_member`
12. `project`

### Scheduler Tasks

Add to the existing scheduler loop (runs every 30s), with daily checks:

```rust
// Once per day (track last check time):
async fn check_workspace_inactivity(state: &AppState) {
    // 1. Find workspaces with no member login in 90 days (exclude protected, already suspended)
    // 2. For each: send warning email if not already sent, record in workspace_warning

    // 3. Find workspaces with warning sent 30+ days ago and still no activity
    // 4. Soft-delete: set deleted_at = now()

    // 5. Find suspended workspaces where deleted_at is 360+ days ago (5 days before 1 year)
    // 6. Send system admin warning email if not already sent

    // 7. Find suspended workspaces where deleted_at is 365+ days ago
    // 8. Hard delete
}
```

---

## 6. Email Deliverability (Custom Domain)

### Problem

Azure Managed Domain sender (`DoNotReply@xxx.azurecomm.net`) goes to junk folder.

### Solution

Configure custom domain `alethedash.com` in ACS:

1. Add ACS Email custom domain for `alethedash.com`
2. Add DNS records:
   - SPF: `TXT` record on `alethedash.com` — `v=spf1 include:spf.protection.outlook.com -all`
   - DKIM: `CNAME` records provided by ACS
   - DMARC: `TXT` record — `v=DMARC1; p=reject; rua=mailto:admin@alethedash.com`
3. Verify domain in ACS
4. Update `DASHBOARD_ACS_SENDER` to `noreply@alethedash.com`

### Email Templates

All emails use plain text (matching the terminal aesthetic):

- **Workspace invite**: Subject "You've been invited to [Name] — AletheDash"
- **Password reset**: Subject "AletheDash — Password Reset" (existing)
- **Inactivity warning (90d)**: Subject "AletheDash — [Name] workspace will be suspended"
- **Inactivity reminder (7d left)**: Subject "AletheDash — [Name] suspended in 7 days"
- **Hard delete warning (system admin)**: Subject "AletheDash — [Name] permanent deletion in 5 days"
- **Deletion summary (system admin)**: Subject "AletheDash — Workspace [Name] permanently deleted"

---

## 7. Frontend Changes Summary

### New Pages

| Page | Route | Access |
|---|---|---|
| System Dashboard | `/admin/system` | System admin only |
| Accept Invite | `/invite/:token` | Public (no auth) |

### Modified Pages

| Page | Changes |
|---|---|
| Sidebar | Add "System" nav item under PLATFORM for system admins |
| Members Page | Add pending invites section, change "Add Member" to "Invite", add invite form |
| Workspaces Page | Show suspended workspace indicator |
| Login Page | After accepting invite, redirect to the workspace |

### New Components

| Component | Purpose |
|---|---|
| `SystemOverviewTab` | CPU/memory/disk/DB metrics with auto-refresh |
| `WorkspaceUsageTab` | Table with suspend/protect/delete actions |
| `SystemLogsTab` | Filterable log viewer with auto-scroll |
| `InviteForm` | Email + role input for workspace invite |
| `AcceptInvitePage` | Standalone page for invite acceptance |

---

## 8. API Endpoints Summary

### New Endpoints

**System admin (no workspace scope):**
```
GET    /api/admin/system/metrics           — server + DB metrics
GET    /api/admin/system/usage             — workspace usage table
GET    /api/admin/system/logs              — filtered log entries
POST   /api/admin/workspaces/:pid/suspend  — soft-delete workspace
POST   /api/admin/workspaces/:pid/restore  — restore suspended workspace
POST   /api/admin/workspaces/:pid/protect  — toggle delete protection
DELETE /api/admin/workspaces/:pid          — hard delete (permanent)
```

**Project-scoped (workspace admin):**
```
GET    /api/projects/:pid/invites          — list invites
POST   /api/projects/:pid/invites          — create invite
DELETE /api/projects/:pid/invites/:iid     — revoke invite
```

**Public (no auth):**
```
GET    /api/invite/:token                  — resolve invite metadata
POST   /api/invite/:token/accept           — accept invite
```

---

## 9. Migration Plan

### V012 Migration

```sql
-- Workspace invite table
CREATE TABLE IF NOT EXISTS workspace_invite ( ... );

-- Workspace warning tracking
CREATE TABLE IF NOT EXISTS workspace_warning ( ... );

-- Soft delete columns on project
ALTER TABLE project ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;
ALTER TABLE project ADD COLUMN IF NOT EXISTS delete_protection BOOLEAN NOT NULL DEFAULT FALSE;

-- Protect the Default workspace
UPDATE project SET delete_protection = TRUE WHERE project_id = '00000000-0000-0000-0000-000000000001';
```

---

## 10. Implementation Order

1. **PR 1: V012 migration + invite backend** — DB tables, invite CRUD, accept flow
2. **PR 2: Invite frontend** — Members page invite UI, accept invite page
3. **PR 3: System metrics backend** — sysinfo, pg_stat, log buffer, API endpoints
4. **PR 4: System admin dashboard frontend** — Overview, Usage, Logs tabs
5. **PR 5: Soft delete + inactivity** — Scheduler tasks, warning emails, suspend/restore API
6. **PR 6: Email custom domain** — ACS configuration, DNS records, template updates
7. **PR 7: Hardening** — Version bump to v0.16.0, cleanup

---

## Non-Goals (deferred)

- Self-service workspace creation by users
- Billing / usage quotas per workspace
- Audit log (detailed activity tracking beyond system logs)
- Webhook notifications (Slack, Teams)
- Custom email templates (HTML)
- Multi-region deployment
