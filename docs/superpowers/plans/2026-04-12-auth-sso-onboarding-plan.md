# Auth/SSO Configuration + User Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace env-var SSO configuration with DB-stored dynamic providers, add batch CSV user import with invite tracking, and add pending-project acceptance flow on login.

**Architecture:** Three independent subsystems — (A) SSO provider CRUD + runtime cache replacing env vars, (B) batch CSV import + membership status on project_member, (C) post-login pending-project modal. Each phase produces deployable, testable code.

**Tech Stack:** Rust (axum, tokio-postgres, aes-gcm), React + TypeScript (Vite, Tailwind), PostgreSQL

**Source spec:** `docs/superpowers/specs/2026-04-12-auth-sso-onboarding-design.md`

---

## Build Order

10 tasks in 3 phases. Each phase produces compilable, testable code.

1. **Phase A — SSO Provider Management** (Tasks 1–4): Migration, DB layer, API, refactor SSO flow, frontend Auth tab.
2. **Phase B — Batch Import + Membership Status** (Tasks 5–7): Migration, import API, frontend members upgrade.
3. **Phase C — Post-Login Pending Projects** (Tasks 8–10): API, frontend modal, version bump + validation.

---

## Phase A — SSO Provider Management

### Task 1: V030 migration — sso_provider + system_config + project_member status

**Goal:** Create the new tables and add `status` column to `project_member`.

**Files:**
- Modify `crates/networker-dashboard/src/db/migrations.rs`

The migration adds three things:

```sql
-- V030: Dynamic SSO providers, system config, and project member status.

-- 1. SSO providers (replaces env-var SSO config)
CREATE TABLE IF NOT EXISTS sso_provider (
    provider_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                 VARCHAR(200) NOT NULL,
    provider_type        VARCHAR(30)  NOT NULL,
    client_id            TEXT         NOT NULL,
    client_secret_enc    BYTEA        NOT NULL,
    client_secret_nonce  BYTEA        NOT NULL,
    issuer_url           TEXT,
    tenant_id            TEXT,
    extra_config         JSONB        NOT NULL DEFAULT '{}',
    enabled              BOOLEAN      NOT NULL DEFAULT TRUE,
    display_order        SMALLINT     NOT NULL DEFAULT 0,
    created_by           UUID         REFERENCES dash_user(user_id),
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- 2. Platform-level system config (public_url, etc.)
CREATE TABLE IF NOT EXISTS system_config (
    key         VARCHAR(100) PRIMARY KEY,
    value       TEXT         NOT NULL,
    updated_by  UUID         REFERENCES dash_user(user_id),
    updated_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

-- 3. Project member status + invite tracking
ALTER TABLE project_member
  ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'active',
  ADD COLUMN IF NOT EXISTS invite_sent_at TIMESTAMPTZ;
```

Follow the existing migration runner pattern (V028/V029).

- [ ] Step 1: Add `V030_SSO_AND_MEMBER_STATUS` constant with the SQL above
- [ ] Step 2: Add the V030 runner block after V029 in the migration runner function
- [ ] Step 3: `cargo build -p networker-dashboard` clean
- [ ] Step 4: Commit: `feat(dashboard): V030 sso_provider + system_config + member status`

---

### Task 2: SSO provider DB layer + CRUD API

**Goal:** Create the DB helpers and REST endpoints for SSO provider management (platform admin only).

**Files:**
- Create `crates/networker-dashboard/src/db/sso_providers.rs`
- Modify `crates/networker-dashboard/src/db/mod.rs` (add `pub mod sso_providers;`)
- Create `crates/networker-dashboard/src/api/sso_admin.rs`
- Modify `crates/networker-dashboard/src/api/mod.rs` (add `pub mod sso_admin;`)
- Modify `crates/networker-dashboard/src/main.rs` (wire router)

**DB layer (`sso_providers.rs`):**

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct SsoProviderRow {
    pub provider_id: Uuid,
    pub name: String,
    pub provider_type: String,
    pub client_id: String,
    pub client_secret_enc: Vec<u8>,
    pub client_secret_nonce: Vec<u8>,
    pub issuer_url: Option<String>,
    pub tenant_id: Option<String>,
    pub extra_config: serde_json::Value,
    pub enabled: bool,
    pub display_order: i16,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSsoProvider {
    pub name: String,
    pub provider_type: String,
    pub client_id: String,
    pub client_secret: String,     // plaintext — encrypted before storage
    pub issuer_url: Option<String>,
    pub tenant_id: Option<String>,
    pub extra_config: Option<serde_json::Value>,
    pub enabled: Option<bool>,
    pub display_order: Option<i16>,
}
```

Implement: `list_all`, `get_by_id`, `insert`, `update`, `delete`.

The `insert` and `update` functions accept plaintext `client_secret` — the API layer encrypts it before calling the DB layer. Alternatively, store the encrypted bytes directly. Choose whichever matches the cloud_accounts pattern. The cloud_accounts module stores encrypted bytes in the row struct and the API layer handles encrypt/decrypt.

**API layer (`sso_admin.rs`):**

Endpoints (all require `is_platform_admin`):
```
GET    /api/admin/sso-providers           — list (secrets redacted)
POST   /api/admin/sso-providers           — create
PUT    /api/admin/sso-providers/{id}      — update
DELETE /api/admin/sso-providers/{id}      — delete
```

The list/get responses use a `SsoProviderResponse` that has `has_client_secret: bool` instead of the actual secret bytes.

On create/update, encrypt `client_secret` using `crate::crypto::encrypt(secret.as_bytes(), key)` where `key` comes from `state.credential_key`. On update, if `client_secret` is omitted/null in the body, keep the existing encrypted value.

After any CRUD operation, refresh the in-memory SSO provider cache (Task 3 wires this up).

**Validation:**
- `provider_type` must be one of: `"microsoft"`, `"google"`, `"oidc_generic"`
- `microsoft` requires `tenant_id` non-empty
- `oidc_generic` requires `issuer_url` non-empty and starting with `https://`
- `name` must be non-empty

**Router wiring in `main.rs`:**
Nest the SSO admin routes under the existing admin router section. Look at how `/api/admin/system` or `/api/users` are wired.

Unit tests:
- `validation_rejects_missing_tenant_for_microsoft`
- `validation_rejects_http_issuer_for_oidc`
- `validation_accepts_valid_google_config`

- [ ] Step 1: Create `sso_providers.rs` with row struct, create input, and DB helpers
- [ ] Step 2: Wire into `db/mod.rs`
- [ ] Step 3: Create `sso_admin.rs` with CRUD endpoints
- [ ] Step 4: Wire into `api/mod.rs` and `main.rs` router
- [ ] Step 5: Add validation unit tests
- [ ] Step 6: `cargo build -p networker-dashboard` and `cargo test -p networker-dashboard --lib sso` clean
- [ ] Step 7: Commit: `feat(dashboard): SSO provider CRUD API (platform admin)`

---

### Task 3: Refactor SSO flow to use DB providers instead of env vars

**Goal:** Replace the hardcoded env-var SSO config in the auth flow with the DB-backed provider cache.

**Files:**
- Modify `crates/networker-dashboard/src/main.rs` (add SSO provider cache to AppState)
- Modify `crates/networker-dashboard/src/api/auth.rs` (refactor sso_init, sso_callback, sso_providers)

**AppState changes:**

Add a new field to `AppState`:
```rust
pub sso_provider_cache: tokio::sync::RwLock<Vec<db::sso_providers::SsoProviderRow>>,
```

On startup, after DB migrations run, load all enabled providers:
```rust
let providers = db::sso_providers::list_enabled(&client).await?;
// ... build AppState with sso_provider_cache: RwLock::new(providers)
```

Add a helper method to AppState (or a free function):
```rust
pub async fn refresh_sso_cache(state: &AppState, client: &Client) -> anyhow::Result<()> {
    let providers = db::sso_providers::list_enabled(client).await?;
    let mut cache = state.sso_provider_cache.write().await;
    *cache = providers;
    Ok(())
}
```

Call `refresh_sso_cache` after every create/update/delete in `sso_admin.rs`.

**Backwards compatibility on startup:**

If env vars `SSO_MICROSOFT_CLIENT_ID` etc. are set AND no `sso_provider` rows exist for that type, auto-create a provider row. This is a one-time migration from env vars to DB. Do this after migrations run, before building AppState. Log a message: `"Migrated SSO config from env vars to database"`.

**Refactor `sso_providers` endpoint (`GET /api/auth/sso/providers`):**

Currently returns `["microsoft", "google"]` based on env vars. Change to read from cache:
```rust
let cache = state.sso_provider_cache.read().await;
let providers: Vec<_> = cache.iter()
    .filter(|p| p.enabled)
    .map(|p| json!({ "id": p.provider_id, "name": p.name, "type": p.provider_type }))
    .collect();
```

Response shape changes from `{ providers: ["microsoft"] }` to `{ providers: [{ id, name, type }] }`.

**Refactor `sso_init` (`GET /api/auth/sso/init`):**

Currently takes `?provider=microsoft`. Change to `?provider={provider_id}` (UUID).

Look up the provider from cache by ID. Build the OAuth URL using the provider's config:
- `microsoft`: use `provider.tenant_id` for the URL template
- `google`: hardcoded endpoints
- `oidc_generic`: fetch `{issuer_url}/.well-known/openid-configuration` to discover endpoints (cache the discovery response)

Decrypt the client_secret from the provider row using `crypto::decrypt`.

The CSRF state format changes from `"microsoft:{random}"` to `"{provider_id}:{random}"`.

**Refactor `sso_callback`:**

Extract `provider_id` from the state string (first segment before `:`). Look up provider from cache. Use it for token exchange (client_id, decrypted client_secret, token endpoint) and ID token validation (issuer, audience).

**Keep existing env-var fields on AppState** for now (they're used by the backwards-compat migration). They can be removed in a future cleanup PR once all deployments have migrated.

- [ ] Step 1: Add `list_enabled` to `db/sso_providers.rs`
- [ ] Step 2: Add `sso_provider_cache` to AppState, load on startup
- [ ] Step 3: Add env-var-to-DB migration logic on startup
- [ ] Step 4: Refactor `sso_providers` endpoint to read from cache
- [ ] Step 5: Refactor `sso_init` to use provider_id and DB config
- [ ] Step 6: Refactor `sso_callback` to use provider_id and DB config
- [ ] Step 7: Add `refresh_sso_cache` call in sso_admin CRUD handlers
- [ ] Step 8: `cargo build -p networker-dashboard` clean
- [ ] Step 9: `cargo test -p networker-dashboard --lib` passes
- [ ] Step 10: Commit: `refactor(dashboard): SSO flow uses DB providers instead of env vars`

---

### Task 4: Frontend — System Admin Auth tab + login page update

**Goal:** Add the Auth tab to the System admin page for SSO provider CRUD, and update the login page to render dynamic provider buttons.

**Files:**
- Modify `dashboard/src/pages/SystemDashboardPage.tsx` (add 'auth' tab)
- Modify `dashboard/src/pages/LoginPage.tsx` (render dynamic buttons)
- Modify `dashboard/src/api.ts` (add SSO admin API calls)

**SystemDashboardPage.tsx changes:**

Add `'auth'` to the `Tab` type: `type Tab = 'overview' | 'usage' | 'logs' | 'auth'`

Add the tab button alongside existing ones.

Add an `AuthTab` component that renders:
1. **Public URL** section: text input bound to `system_config.public_url`, Save button. Fetch/save via new endpoints `GET/PUT /api/admin/system-config/public_url`.
2. **SSO Providers** section:
   - Table: Name, Type, Enabled (toggle), Actions (Edit, Delete)
   - "+ Add Provider" button opens a modal/form
   - The form shows a type dropdown. When type is selected, render type-specific fields using a `PROVIDER_FIELDS` config map:

```tsx
const PROVIDER_FIELDS: Record<string, FieldDef[]> = {
  microsoft: [
    { key: 'client_id', label: 'Application (client) ID', required: true },
    { key: 'client_secret', label: 'Client Secret', required: true, secret: true },
    { key: 'tenant_id', label: 'Directory (tenant) ID', required: true,
      help: 'Azure Portal > App Registrations > Overview' },
  ],
  google: [
    { key: 'client_id', label: 'Client ID', required: true },
    { key: 'client_secret', label: 'Client Secret', required: true, secret: true,
      help: 'Google Cloud Console > APIs & Services > Credentials' },
  ],
  oidc_generic: [
    { key: 'client_id', label: 'Client ID', required: true },
    { key: 'client_secret', label: 'Client Secret', required: true, secret: true },
    { key: 'issuer_url', label: 'Issuer URL', required: true,
      help: 'Must support .well-known/openid-configuration' },
  ],
};
```

Edit mode: pre-fill fields, show client_secret as placeholder "••••••••", only send if changed.

**LoginPage.tsx changes:**

The SSO providers response shape changed from `{ providers: ["microsoft"] }` to `{ providers: [{ id, name, type }] }`.

Update the provider rendering to show a button per provider with the provider's `name`:
```tsx
{providers.map(p => (
  <a key={p.id} href={`/api/auth/sso/init?provider=${p.id}`}
     className="...">
    Continue with {p.name}
  </a>
))}
```

**api.ts changes:**

Add functions:
```typescript
// SSO admin
getSsoProviders(): Promise<SsoProvider[]>
createSsoProvider(data: CreateSsoProvider): Promise<SsoProvider>
updateSsoProvider(id: string, data: UpdateSsoProvider): Promise<SsoProvider>
deleteSsoProvider(id: string): Promise<void>

// System config
getSystemConfig(key: string): Promise<string | null>
setSystemConfig(key: string, value: string): Promise<void>
```

Also add a simple `GET/PUT /api/admin/system-config/{key}` endpoint pair in the backend (small addition to `api/admin.rs` or a new `api/system_config.rs`).

- [ ] Step 1: Add system_config API endpoints (backend)
- [ ] Step 2: Add SSO admin + system config API functions to `api.ts`
- [ ] Step 3: Add Auth tab to SystemDashboardPage with provider CRUD form
- [ ] Step 4: Update LoginPage to render dynamic provider buttons
- [ ] Step 5: `cd dashboard && npm run build && npm run lint` clean
- [ ] Step 6: `cargo build -p networker-dashboard` clean
- [ ] Step 7: Commit: `feat: SSO provider management UI + dynamic login buttons`

---

## Phase B — Batch Import + Membership Status

### Task 5: Project member status + import API

**Goal:** Add status/invite_sent_at to the member DB layer and create the CSV import endpoint.

**Files:**
- Modify `crates/networker-dashboard/src/db/projects.rs` (add status to ProjectMemberRow, new import helpers)
- Create `crates/networker-dashboard/src/api/member_import.rs`
- Modify `crates/networker-dashboard/src/api/mod.rs`
- Modify `crates/networker-dashboard/src/main.rs` (wire router)

**DB layer changes (`projects.rs`):**

Add to `ProjectMemberRow`:
```rust
pub status: String,           // 'active', 'pending_acceptance', 'denied'
pub invite_sent_at: Option<DateTime<Utc>>,
```

Update `list_members` SELECT to include the new columns.

Add `add_pending_member` function:
```rust
pub async fn add_pending_member(
    client: &Client,
    project_id: &str,
    user_id: &Uuid,
    role: &str,
    invited_by: &Uuid,
) -> anyhow::Result<AddMemberResult>
```

Where `AddMemberResult` is an enum: `Added`, `AlreadyMember`, `AlreadyPending`, `ReInvited` (for denied → pending_acceptance reset).

Add `update_member_status` function for accept/deny.

**Import endpoint (`member_import.rs`):**

```
POST /api/projects/{pid}/members/import
Content-Type: multipart/form-data
Body: file=<CSV>
```

Requires project role `admin`.

Parse CSV (email, role). For each row:
1. Validate email format and role value
2. Look up user by email — if not found, create placeholder (`status='active'`, `password_hash=NULL`, `auth_provider=NULL`, `must_change_password=FALSE`)
3. Call `add_pending_member` to add project membership with `status='pending_acceptance'`
4. Collect results

Return:
```json
{
  "imported": 5,
  "skipped": 2,
  "errors": 0,
  "details": [
    {"email": "john@co.com", "result": "invited", "message": "New user created + invited as operator"},
    {"email": "jane@co.com", "result": "already_member", "message": "Already a viewer in this project"}
  ]
}
```

Use the `csv` crate for parsing (add to Cargo.toml if not present, or parse manually with `split(',')`).

For creating placeholder users, add a `create_placeholder_user` function to `db/users.rs`:
```rust
pub async fn create_placeholder_user(client: &Client, email: &str) -> anyhow::Result<Uuid>
```
This inserts a user with `status='active'`, `password_hash=NULL`, all other fields default. If a user with that email already exists, return their existing user_id.

Unit tests:
- `add_pending_member_result_variants` (compile-check the enum)
- CSV parsing edge cases (empty lines, bad role values, duplicate emails)

- [ ] Step 1: Add `status` and `invite_sent_at` to `ProjectMemberRow` and queries
- [ ] Step 2: Add `add_pending_member` and `update_member_status` functions
- [ ] Step 3: Add `create_placeholder_user` to `db/users.rs`
- [ ] Step 4: Create `member_import.rs` with CSV import endpoint
- [ ] Step 5: Wire into router
- [ ] Step 6: Add unit tests for CSV parsing
- [ ] Step 7: `cargo build -p networker-dashboard` and `cargo test -p networker-dashboard --lib` clean
- [ ] Step 8: Commit: `feat(dashboard): batch CSV user import + member status`

---

### Task 6: Invite email send/resend for imported members

**Goal:** Project admins can send/resend invite emails to pending members.

**Files:**
- Modify `crates/networker-dashboard/src/api/member_import.rs` (add send/resend endpoints)
- Modify `crates/networker-dashboard/src/db/projects.rs` (update invite_sent_at)

**New endpoints:**

```
POST /api/projects/{pid}/members/send-invites
Body: { "user_ids": ["uuid1", "uuid2", ...] }
```

Requires project role `admin`. For each user_id:
1. Verify they're a pending member of this project
2. Generate an invite token (using existing `workspace_invite` flow)
3. Create a `workspace_invite` row (or reuse existing pending one)
4. Send email with invite link (using existing `crate::email::send_email`)
5. Update `project_member.invite_sent_at = NOW()`

Return summary: `{ "sent": 3, "skipped": 1, "errors": 0 }`.

If no email service is configured (`state.email_sender` is None), return 400 with a clear message instead of silently failing.

- [ ] Step 1: Add send-invites endpoint
- [ ] Step 2: Update `invite_sent_at` in DB on send
- [ ] Step 3: `cargo build -p networker-dashboard` clean
- [ ] Step 4: Commit: `feat(dashboard): send/resend invite emails for imported members`

---

### Task 7: Frontend — Members page upgrade (status, import, bulk invite)

**Goal:** Add CSV import, status column, and bulk invite actions to the Members page.

**Files:**
- Modify `dashboard/src/pages/ProjectMembersPage.tsx`
- Modify `dashboard/src/api.ts` (add import + send-invites API calls)

**Members page changes:**

1. **Status column**: Show badge next to each member — green "active", yellow "pending", red "denied". The status comes from the updated `list_members` response.

2. **Filter dropdown**: "All / Active / Pending / Denied" above the table.

3. **"Import Users" button**: Opens a modal with:
   - File input for CSV upload
   - Preview table showing parsed rows before import
   - "Import" button that calls `POST /api/projects/{pid}/members/import`
   - Results summary after import

4. **Checkboxes + "Send Invite Email" button**: For pending members, show checkboxes. When selected, enable a "Send Invite Email" button that calls the send-invites endpoint.

5. **"Resend" action**: Per-row action for pending members who have `invite_sent_at` set. Shows "Resent" vs "Send" based on whether invite was already sent.

**api.ts additions:**
```typescript
importMembers(projectId: string, file: File): Promise<ImportResult>
sendInvites(projectId: string, userIds: string[]): Promise<SendResult>
```

- [ ] Step 1: Add import + send-invites API functions to `api.ts`
- [ ] Step 2: Add status column + filter to members table
- [ ] Step 3: Add CSV import modal
- [ ] Step 4: Add bulk invite checkboxes + send button
- [ ] Step 5: `cd dashboard && npm run build && npm run lint` clean
- [ ] Step 6: Commit: `feat: members page CSV import + status + bulk invite`

---

## Phase C — Post-Login Pending Projects

### Task 8: Pending projects API

**Goal:** API endpoints for listing and responding to pending project memberships.

**Files:**
- Create `crates/networker-dashboard/src/api/pending_projects.rs`
- Modify `crates/networker-dashboard/src/api/mod.rs`
- Modify `crates/networker-dashboard/src/main.rs` (wire router)

**Endpoints (require valid JWT only, no project role):**

```
GET    /api/me/pending-projects               — list pending memberships
PUT    /api/projects/{pid}/members/me/accept   — accept
PUT    /api/projects/{pid}/members/me/deny     — deny
```

**GET /api/me/pending-projects** response:
```json
{
  "pending": [
    {
      "project_id": "us057ygm4a200q",
      "project_name": "Pre-Prod Testing",
      "role": "operator",
      "invited_by_email": "admin@company.com",
      "invited_at": "2026-04-12T..."
    }
  ]
}
```

Query:
```sql
SELECT pm.project_id, p.name, pm.role, u.email as invited_by_email, pm.joined_at
FROM project_member pm
JOIN project p ON p.project_id = pm.project_id
LEFT JOIN dash_user u ON u.user_id = pm.invited_by
WHERE pm.user_id = $1 AND pm.status = 'pending_acceptance'
```

**PUT accept**: Update `project_member SET status = 'active' WHERE project_id = $1 AND user_id = $2 AND status = 'pending_acceptance'`.

**PUT deny**: Update `project_member SET status = 'denied' WHERE project_id = $1 AND user_id = $2 AND status = 'pending_acceptance'`.

Both return 404 if no pending membership found.

- [ ] Step 1: Create `pending_projects.rs` with the 3 endpoints
- [ ] Step 2: Wire into router (these are NOT project-scoped — they sit at `/api/me/...` and `/api/projects/{pid}/members/me/...`)
- [ ] Step 3: `cargo build -p networker-dashboard` clean
- [ ] Step 4: `cargo test -p networker-dashboard --lib` passes
- [ ] Step 5: Commit: `feat(dashboard): pending project membership API`

---

### Task 9: Frontend — Post-login pending projects modal

**Goal:** Show a modal after login if the user has pending project invitations.

**Files:**
- Create `dashboard/src/components/PendingProjectsModal.tsx`
- Modify `dashboard/src/pages/SSOCompletePage.tsx` (check pending after SSO login)
- Modify `dashboard/src/pages/LoginPage.tsx` (check pending after password login)
- Modify `dashboard/src/api.ts` (add pending projects API calls)

**PendingProjectsModal component:**

A modal that receives a list of pending projects and renders:
- Project name, role, invited-by for each
- Accept / Deny / Ignore buttons per project
- Accept calls `PUT /api/projects/{pid}/members/me/accept`
- Deny calls `PUT /api/projects/{pid}/members/me/deny`
- Ignore just removes from the local list (no API call — will show again next login)
- "Continue" button to dismiss modal

**Integration points:**

After successful login (both password and SSO paths), fetch `GET /api/me/pending-projects`. If the `pending` array is non-empty, show the modal before navigating to the default project.

In `SSOCompletePage.tsx`, after the `ssoExchange` call succeeds and before the navigation logic, insert the pending check.

In `LoginPage.tsx` (or the auth store's login flow), after successful password login, insert the pending check.

**api.ts additions:**
```typescript
getPendingProjects(): Promise<PendingProject[]>
acceptProject(projectId: string): Promise<void>
denyProject(projectId: string): Promise<void>
```

- [ ] Step 1: Add pending projects API functions to `api.ts`
- [ ] Step 2: Create `PendingProjectsModal.tsx`
- [ ] Step 3: Integrate into SSOCompletePage after exchange
- [ ] Step 4: Integrate into LoginPage after password login
- [ ] Step 5: `cd dashboard && npm run build && npm run lint` clean
- [ ] Step 6: Commit: `feat: post-login pending projects modal`

---

### Task 10: Version bump + CHANGELOG + final validation

**Goal:** Bump version, add CHANGELOG entry, run full validation.

**Files:**
- Modify `Cargo.toml` (workspace version)
- Modify `install.sh` (INSTALLER_VERSION)
- Modify `install.ps1` (InstallerVersion)
- Modify `CHANGELOG.md`

Version bump: current version → next minor (check current version at implementation time).

CHANGELOG entry:
```markdown
## [X.Y.Z] — YYYY-MM-DD

### Added
- **Dynamic SSO provider management**: Configure Microsoft, Google, and generic OIDC
  providers from the System Admin UI. No env vars or restarts needed.
- **Batch CSV user import**: Project admins import users by email + role via CSV upload.
  Imported users see pending project invitations on login.
- **Post-login project acceptance**: Users with pending project memberships see an
  accept/deny/ignore prompt after login.
- V030 migration: `sso_provider`, `system_config` tables; `status` column on `project_member`.

### Changed
- SSO configuration moved from env vars to database (backwards compatible — existing
  env vars auto-migrate to DB on first startup).
- Login page SSO buttons rendered dynamically from configured providers.
```

**Final validation checklist:**
- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo test --workspace --lib` passes
- [ ] `cargo build --workspace` clean
- [ ] `cargo build -p networker-tester --no-default-features` clean
- [ ] `cd dashboard && npm run build && npm run lint` clean
- [ ] `shellcheck install.sh` clean

- [ ] Step 1: Version bump (3 locations)
- [ ] Step 2: CHANGELOG entry
- [ ] Step 3: Run full validation checklist
- [ ] Step 4: Commit: `feat: auth/SSO + user onboarding (vX.Y.Z)`
