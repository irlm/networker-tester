# v0.14 Auth Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace username-based auth with email identity, add Microsoft/Google SSO, implement three-tier RBAC (admin/operator/viewer), pending user approval, Azure Communication Services email, and remove local tester auto-spawn.

**Architecture:** Seven PRs in dependency order. Each PR is independently deployable. The auth middleware is the critical path — PR 1 (email identity) and PR 2 (role enforcement) must land before SSO (PR 3) and pending approval (PR 4). Email service (PR 5) and tester removal (PR 6) are independent. Error page + CLI setup (PR 7) is the final cleanup.

**Tech Stack:** Rust (axum 0.7, tokio-postgres, bcrypt, jsonwebtoken), React 19 (TypeScript, Zustand, Tailwind), PostgreSQL 16, Azure Communication Services, OAuth 2.0 (Microsoft Entra ID + Google)

**Spec:** `docs/superpowers/specs/2026-03-21-v014-auth-overhaul-design.md`

---

## File Map

### Files to Create
| File | PR | Purpose |
|------|-----|---------|
| `crates/networker-dashboard/src/db/sso.rs` | 3 | SSO user lookup/creation, account linking |
| `crates/networker-dashboard/src/api/users.rs` | 2 | Admin user management endpoints |
| `crates/networker-dashboard/src/email.rs` | 5 | Azure Communication Services email client |
| `dashboard/src/pages/UsersPage.tsx` | 2 | Admin user management UI |
| `dashboard/src/pages/PendingPage.tsx` | 4 | Pending approval page |
| `dashboard/src/pages/SSOCompletePage.tsx` | 3 | SSO code-to-JWT exchange (needed to complete OAuth flow) |
| `dashboard/src/pages/ConfigErrorPage.tsx` | 7 | Missing env var error page |

### Files to Modify (by PR)

**PR 1:** `config.rs`, `db/migrations.rs`, `db/users.rs`, `auth/mod.rs`, `api/auth.rs`, `api/mod.rs`, `main.rs`, `stores/authStore.ts`, `pages/LoginPage.tsx`, `pages/ChangePasswordPage.tsx`, `api/client.ts`, `api/types.ts`

**PR 2:** `auth/mod.rs`, `api/mod.rs`, `db/users.rs`, `App.tsx`, `Sidebar.tsx`

**PR 3:** `config.rs`, `Cargo.toml` (add `tower-governor`), `api/auth.rs`, `api/mod.rs`, `db/mod.rs`, `pages/LoginPage.tsx`, `pages/SSOCompletePage.tsx` (create), `api/client.ts`, `stores/authStore.ts`, `App.tsx`

**PR 4:** `auth/mod.rs`, `api/auth.rs`, `db/users.rs`, `App.tsx`, `stores/authStore.ts`

**PR 5:** `Cargo.toml`, `api/auth.rs`, `config.rs`

**PR 6:** `main.rs`, `pages/JobsPage.tsx`

**PR 7:** `main.rs`, `config.rs`, `App.tsx`

---

## Task 1: Email Identity + V008 Migration (PR 1)

**Files:**
- Modify: `crates/networker-dashboard/src/config.rs`
- Modify: `crates/networker-dashboard/src/db/migrations.rs`
- Modify: `crates/networker-dashboard/src/db/users.rs`
- Modify: `crates/networker-dashboard/src/auth/mod.rs`
- Modify: `crates/networker-dashboard/src/api/auth.rs`
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `dashboard/src/stores/authStore.ts`
- Modify: `dashboard/src/pages/LoginPage.tsx`
- Modify: `dashboard/src/pages/ChangePasswordPage.tsx`
- Modify: `dashboard/src/api/client.ts`

- [ ] **Step 1: Add `DASHBOARD_ADMIN_EMAIL` to config.rs**

In `config.rs`, add `admin_email: Option<String>` to `DashboardConfig`. Read from `DASHBOARD_ADMIN_EMAIL` env var. This replaces `admin_password` as the primary admin seed identifier.

```rust
pub admin_email: Option<String>,  // DASHBOARD_ADMIN_EMAIL
```

Run: `cargo check -p networker-dashboard`

- [ ] **Step 2: Write V008 migration SQL**

In `db/migrations.rs`, add `V008_AUTH_OVERHAUL` constant:

```sql
-- Step 1: Backfill email from username for existing users (MUST run before dropping username)
UPDATE dash_user SET email = username WHERE email IS NULL OR email = '';

-- Step 2: Now safe to enforce NOT NULL + UNIQUE on email
ALTER TABLE dash_user ALTER COLUMN email SET NOT NULL;
ALTER TABLE dash_user ADD CONSTRAINT dash_user_email_unique UNIQUE (email);

-- Step 3: Now safe to drop username
ALTER TABLE dash_user DROP COLUMN IF EXISTS username;

-- Step 4: Allow NULL password for SSO-only accounts
ALTER TABLE dash_user ALTER COLUMN password_hash DROP NOT NULL;

-- Step 5: Add new columns
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'pending';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS auth_provider VARCHAR(20) NOT NULL DEFAULT 'local';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_subject_id VARCHAR(255);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS display_name VARCHAR(200);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS avatar_url TEXT;
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_only BOOLEAN NOT NULL DEFAULT FALSE;

-- Step 6: Migrate disabled → status (preserves existing user state)
UPDATE dash_user SET status = 'active' WHERE disabled = FALSE;
UPDATE dash_user SET status = 'disabled' WHERE disabled = TRUE;
ALTER TABLE dash_user DROP COLUMN IF EXISTS disabled;

-- Step 7: Invalidate existing plaintext reset tokens (will be re-issued as SHA-256)
UPDATE dash_user SET password_reset_token = NULL, password_reset_expires = NULL;

-- Step 8: Indexes
CREATE INDEX IF NOT EXISTS ix_user_sso ON dash_user (auth_provider, sso_subject_id) WHERE sso_subject_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_user_status ON dash_user (status);
```

**CRITICAL ordering**: backfill email BEFORE dropping username, enforce NOT NULL BEFORE dropping username. The SQL above runs in the correct order inside a single `V008_AUTH_OVERHAUL` constant.

Run: `cargo check -p networker-dashboard`

- [ ] **Step 3: Create Role enum in auth/mod.rs**

Add a Rust enum for roles with serialization:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    pub fn has_permission(&self, required: &Role) -> bool {
        match (self, required) {
            (Role::Admin, _) => true,
            (Role::Operator, Role::Operator | Role::Viewer) => true,
            (Role::Viewer, Role::Viewer) => true,
            _ => false,
        }
    }
}
```

- [ ] **Step 4: Update Claims and AuthUser structs**

In `auth/mod.rs`, replace `username: String` with `email: String` in both `Claims` and `AuthUser`. Update `create_token` and `validate_token` accordingly.

```rust
pub struct Claims {
    pub sub: Uuid,
    pub email: String,
    pub role: Role,  // enum, not String — compile-time safety
    pub exp: usize,
    pub iat: usize,
}

pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
    pub role: Role,
}
```

Run: `cargo check -p networker-dashboard` — expect errors in api/auth.rs (username references)

- [ ] **Step 5: Update db/users.rs — email-based auth**

Rewrite `seed_admin` to take email:
```rust
pub async fn seed_admin(client: &Client, email: &str, password: &str) -> anyhow::Result<()> {
    // Check if any users exist
    // If not, create admin with email, status='active', role='admin', must_change_password=true
}
```

Rewrite `authenticate` to query by email:
```rust
pub async fn authenticate(client: &Client, email: &str, password: &str)
    -> anyhow::Result<Option<(Uuid, String, bool)>> {
    // SELECT ... FROM dash_user WHERE email = $1 AND status = 'active'
    // Also check: if sso_only = true, reject password login
}
```

Update `change_password` signature (already accepts email param from PR #240).

Run: `cargo check -p networker-dashboard`

- [ ] **Step 6: Update api/auth.rs — LoginRequest uses email**

Change `LoginRequest.username` to `LoginRequest.email`. Update the `login` handler to call `authenticate` with email. Update `LoginResponse` to return `email` instead of `username`.

Run: `cargo check -p networker-dashboard`

- [ ] **Step 7: Update main.rs — seed_admin with email**

In the admin seeding block, read `DASHBOARD_ADMIN_EMAIL` from config. Pass it to `seed_admin`:

```rust
if let Some(ref email) = cfg.admin_email {
    db::users::seed_admin(&client, email, &cfg.admin_password).await?;
}
```

Run: `cargo check -p networker-dashboard`

- [ ] **Step 8: Update frontend authStore — username → email**

In `stores/authStore.ts`, replace `username` with `email` everywhere. Update `login()`, `logout()`, localStorage keys.

```typescript
interface AuthState {
  token: string | null;
  email: string | null;  // was: username
  role: string | null;
  status: string | null;  // 'active' | 'pending' — added now for forward compat
  isAuthenticated: boolean;
  mustChangePassword: boolean;
  login: (token: string, email: string, role: string, status: string, mustChange: boolean) => void;
  // ...
}
```

Also update `LoginResponse` in `api/types.ts` to include `status` field. Update `ForgotPasswordPage.tsx` and `ResetPasswordPage.tsx` — these already exist, just need label changes (username→email).

- [ ] **Step 9: Update LoginPage.tsx — email field**

Change the "Username" label to "Email", input type from `text` to `email`, placeholder to `you@company.com`. Update the `handleSubmit` to pass `email` to `api.login`.

- [ ] **Step 10: Update api/client.ts — login uses email**

Change `api.login(username, password)` to `api.login(email, password)`. Update the request body field name.

- [ ] **Step 11: Update ChangePasswordPage.tsx and Sidebar**

In `ChangePasswordPage.tsx`: no functional change needed (already uses email). In `Sidebar.tsx`: replace `username` display with `email` from authStore.

- [ ] **Step 12: Run full test suite**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard
cd dashboard && npm run build
```

- [ ] **Step 13: Commit PR 1**

```bash
git checkout -b feat/v014-email-identity
git add -A
git commit -m "feat(v0.14): email-based identity + V008 migration

- V008 migration: drop username, email as primary identity, add status/auth_provider/sso_subject_id columns, migrate disabled → status
- Role enum (Admin/Operator/Viewer) with permission checking
- seed_admin takes email from DASHBOARD_ADMIN_EMAIL env var
- authenticate/login by email instead of username
- Frontend: authStore, LoginPage, Sidebar all use email"
```

---

## Task 2: Role Enforcement + Users Management (PR 2)

**Files:**
- Modify: `crates/networker-dashboard/src/auth/mod.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Modify: `crates/networker-dashboard/src/db/users.rs`
- Create: `crates/networker-dashboard/src/api/users.rs`
- Create: `dashboard/src/pages/UsersPage.tsx`
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/components/layout/Sidebar.tsx`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/api/types.ts`

- [ ] **Step 1: Add status check to require_auth middleware**

In `auth/mod.rs`, after JWT validation, query the DB for user status. If `status != 'active'`, return 403 with clear message. Exception: allow `/auth/change-password` for users with `must_change_password=true`.

```rust
// After extracting claims from JWT:
let row = client.query_opt(
    "SELECT status FROM dash_user WHERE user_id = $1", &[&claims.sub]
).await?;
if row.map(|r| r.get::<_, String>("status")) != Some("active".into()) {
    return Err(StatusCode::FORBIDDEN); // "Account not active"
}
```

- [ ] **Step 2: Add role-checking helper**

```rust
pub fn require_role(user: &AuthUser, required: Role) -> Result<(), StatusCode> {
    let user_role: Role = serde_json::from_value(
        serde_json::Value::String(user.role.clone())
    ).unwrap_or(Role::Viewer);
    if user_role.has_permission(&required) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}
```

- [ ] **Step 3: Add user management DB functions**

In `db/users.rs`, add:
- `list_users(client) -> Vec<UserRow>` (all users, ordered by status then email)
- `list_pending(client) -> Vec<UserRow>` (status='pending' only)
- `approve_user(client, user_id, role)` (set status='active', role=given)
- `deny_user(client, user_id)` (delete the row or set status='disabled')
- `set_role(client, user_id, role)` (update role)
- `disable_user(client, user_id)` (set status='disabled')
- `get_pending_count(client) -> i64`

- [ ] **Step 4: Create api/users.rs**

New file with admin-only endpoints:
```
GET  /api/users         → list all users
GET  /api/users/pending → list pending + count
POST /api/users/:id/approve  → { role: "operator" }
POST /api/users/:id/deny
PUT  /api/users/:id/role     → { role: "viewer" }
POST /api/users/:id/disable
```

Each handler calls `require_role(user, Role::Admin)?` at the top.

- [ ] **Step 5: Register users router in api/mod.rs**

Add `mod users;` and `.merge(users::router(state.clone()))` to the protected routes.

- [ ] **Step 6: Add User type and API methods to frontend**

In `api/types.ts`:
```typescript
export interface DashUser {
  user_id: string;
  email: string;
  role: string;
  status: string;
  auth_provider: string;
  display_name: string | null;
  last_login_at: string | null;
  created_at: string;
}
```

In `api/client.ts`, add `getUsers`, `getPendingUsers`, `approveUser`, `denyUser`, `setUserRole`, `disableUser` methods.

- [ ] **Step 7: Create UsersPage.tsx**

Unified card list layout (selected in mockup). Features:
- Filter tabs: "Pending (N)" / "All (N)"
- Pending cards: yellow left border, email, provider, time ago, role selector + Approve button + deny ✕
- Active cards: email, provider, last login, role badge (admin=green, operator=cyan, viewer=gray)
- "Invite" button in header (stub for PR 4)
- Polling every 10s via `usePolling`

- [ ] **Step 8: Update App.tsx + Sidebar**

Add `/users` route in `App.tsx` (inside `AuthenticatedApp`, conditionally rendered for admin role).

In `Sidebar.tsx`, add "Users" nav item with icon `👤` — only shown when `role === 'admin'`. Include pending count badge (fetched via a lightweight API call).

- [ ] **Step 9: Apply role guards to existing endpoints**

In relevant API handlers (deployments, agents, schedules, update), add role checks:
- `create_job`, `create_schedule`, `create_deployment`, `create_agent`: require `Operator`
- `cancel_job`, `delete_schedule`, `delete_agent`: require `Operator`
- `update_dashboard`, `update_tester`: require `Admin`
- All GET endpoints: require `Viewer` (any active user)

- [ ] **Step 10: Test + commit PR 2**

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard
cd dashboard && npm run build
git checkout -b feat/v014-role-enforcement
git add -A && git commit -m "feat(v0.14): three-tier RBAC + Users management page"
```

---

## Task 3: SSO Integration — Microsoft + Google (PR 3)

**Files:**
- Modify: `crates/networker-dashboard/src/config.rs`
- Modify: `crates/networker-dashboard/src/api/auth.rs`
- Modify: `crates/networker-dashboard/src/api/mod.rs`
- Create: `crates/networker-dashboard/src/db/sso.rs`
- Modify: `crates/networker-dashboard/src/db/mod.rs`
- Modify: `dashboard/src/pages/LoginPage.tsx`
- Modify: `dashboard/src/api/client.ts`
- Modify: `dashboard/src/stores/authStore.ts`

- [ ] **Step 1: Add SSO config to config.rs**

Add Microsoft (client_id, client_secret, tenant_id) and Google (client_id, client_secret) fields. All optional — SSO buttons hidden if not configured.

- [ ] **Step 2: Create db/sso.rs**

Functions:
- `find_by_sso(client, provider, subject_id) -> Option<UserRow>` — lookup by SSO identity
- `find_by_email(client, email) -> Option<UserRow>` — lookup by email
- `create_sso_user(client, email, provider, subject_id, display_name, avatar_url) -> Uuid` — creates with status='pending'
- `link_sso_to_local(client, user_id, provider, subject_id)` — upgrades auth_provider, sets sso_subject_id
- `store_one_time_code(client, user_id, code, expires) -> ()` — for SSO-to-JWT exchange
- `exchange_code(client, code) -> Option<(Uuid, String, String, String)>` — returns (user_id, email, role, status), deletes code

Add `mod sso;` to `db/mod.rs`.

- [ ] **Step 3: Add SSO endpoints to api/auth.rs**

Public endpoints:
- `GET /auth/providers` — returns `{ microsoft: bool, google: bool }` based on config
- `POST /auth/check-email` — takes `{ email }`, extracts domain, checks if SSO is configured for that domain. Returns `{ provider: "microsoft" | null }`. **Domain-level only, never reveals user existence.**
- `GET /auth/sso/:provider` — constructs OAuth URL, sets state cookie (HttpOnly, SameSite=Lax, 5min TTL), redirects to provider
- `GET /auth/callback/:provider` — validates state cookie, exchanges code for tokens, extracts email/sub from ID token, finds or creates user, stores one-time code, redirects to `/auth/sso-complete?code=<code>`
- `POST /auth/exchange-code` — takes `{ code }`, looks up in DB, returns JWT + user info. Deletes code after use.

- [ ] **Step 4: Implement OAuth URL construction**

Microsoft: `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize?client_id=...&redirect_uri=...&scope=openid email profile&response_type=code&state=...`

Google: `https://accounts.google.com/o/oauth2/v2/auth?client_id=...&redirect_uri=...&scope=openid email profile&response_type=code&state=...`

Redirect URI: `{DASHBOARD_PUBLIC_URL}/auth/callback/{provider}`

- [ ] **Step 5: Implement token exchange**

Microsoft: `POST https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token` with `grant_type=authorization_code&code=...&redirect_uri=...&client_id=...&client_secret=...`

Google: `POST https://oauth2.googleapis.com/token` with same pattern.

Parse the ID token (JWT) to extract `email`, `sub` (subject_id), `name`, `picture`.

- [ ] **Step 6: Implement account linking logic**

In the callback handler, after getting email from SSO:
1. Check `find_by_sso(provider, subject_id)` — if found, proceed (existing SSO user)
2. Check `find_by_email(email)` — if found with `auth_provider = 'local'`:
   - Redirect to a linking page: `/auth/link-account?code=<code>&provider=<provider>`
   - Frontend shows: "This email has a local account. Enter your password to link it with {provider}."
   - On password verification, call `link_sso_to_local`
3. If no user found — create new pending user via `create_sso_user`

- [ ] **Step 7: Redesign LoginPage.tsx**

Card container layout (selected in mockup):
- `GET /auth/providers` on mount → show/hide SSO buttons
- "Continue with Microsoft" → navigates to `/auth/sso/microsoft`
- "Continue with Google" → navigates to `/auth/sso/google`
- "or" divider
- Email input + "Continue" button:
  - Calls `POST /auth/check-email` with the email
  - If returns `{ provider: "microsoft" }` → redirect to `/auth/sso/microsoft?hint=<email>`
  - If returns `{ provider: null }` → show password field inline (slide transition)
- "Forgot password?" link

- [ ] **Step 8: Update api/client.ts + authStore**

Add: `getProviders()`, `checkEmail(email)`, `exchangeCode(code)` methods.

Update `authStore` to track `authProvider` (local/microsoft/google).

- [ ] **Step 9: Create SSOCompletePage.tsx**

Route: `/auth/sso-complete` — reads `code` from URL query params, calls `POST /auth/exchange-code`, stores JWT + email + role + status in authStore, redirects to `/` (active) or `/pending` (pending). Add route in `App.tsx`.

- [ ] **Step 10: Add rate limiting**

Add `tower-governor = "0.4"` to `Cargo.toml`. Wrap auth routes in rate-limiting layer (10 req/min per IP):
- `/auth/login`, `/auth/forgot-password`, `/auth/check-email`, `/auth/sso/:provider`, `/auth/callback/:provider`

```rust
use tower_governor::{GovernorLayer, GovernorConfigBuilder};
let governor_conf = GovernorConfigBuilder::default()
    .per_second(6)  // ~10 per minute
    .burst_size(10)
    .finish().unwrap();
```

- [ ] **Step 11: Add `DASHBOARD_HIDE_SSO_DOMAINS` config**

In `config.rs`, add `hide_sso_domains: bool` from `DASHBOARD_HIDE_SSO_DOMAINS` env var (default false). In `check-email` handler: if `hide_sso_domains`, always return `{ provider: null }`.

- [ ] **Step 12: Test + commit PR 3**

Test: OAuth flows with Microsoft + Google (use test apps in Azure/Google console). Test account linking. Test state parameter validation. Test check-email returns domain-level only. Test rate limiting rejects after burst.

```bash
cargo clippy -p networker-dashboard -- -D warnings
cargo test -p networker-dashboard
cd dashboard && npm run build
git checkout -b feat/v014-sso
git add -A && git commit -m "feat(v0.14): Microsoft + Google SSO with account linking"
```

---

## Task 4: Pending Approval Flow (PR 4)

**Files:**
- Modify: `crates/networker-dashboard/src/auth/mod.rs`
- Modify: `crates/networker-dashboard/src/api/auth.rs`
- Modify: `crates/networker-dashboard/src/api/users.rs` (from PR 2)
- Modify: `crates/networker-dashboard/src/db/users.rs`
- Create: `dashboard/src/pages/PendingPage.tsx`
- Modify: `dashboard/src/pages/SSOCompletePage.tsx` (created in PR 3 — add pending branch)
- Modify: `dashboard/src/App.tsx`
- Modify: `dashboard/src/stores/authStore.ts`

- [ ] **Step 1: Update require_auth for pending users**

In `auth/mod.rs`, after status check: if `status == 'pending'`, still allow JWT but set a flag. The frontend uses this to redirect to `/pending`. Backend rejects all API calls except `/auth/profile` and `/auth/change-password` for pending users.

- [ ] **Step 2: Add status to authStore**

Add `status: 'active' | 'pending' | null` to `AuthState`. Update `login()` to store it. The `LoginResponse` and `exchange-code` response now include `status`.

- [ ] **Step 3: Create PendingPage.tsx**

Centered status layout (selected in mockup):
- Networker branding
- ⏳ icon with yellow border circle
- "Account pending approval" heading
- "Signed in as {email}" in green
- Status card: "Waiting for admin approval"
- "Sign out" button
- Auto-polls `GET /auth/profile` every 10s
- If status changes to 'active', redirects to `/`

- [ ] **Step 5: Update App.tsx routing**

Add routes:
- `/auth/sso-complete` → `<SSOCompletePage />`
- `/pending` → `<PendingPage />`

In `AuthenticatedApp`, check `status`:
- If `status === 'pending'` and not on `/pending` → redirect to `/pending`
- If `status === 'active'` → normal routing

- [ ] **Step 6: Add invite_user to db/users.rs**

```rust
pub async fn invite_user(client: &Client, email: &str, role: &str) -> anyhow::Result<Uuid> {
    // Create user with status='pending', must_change_password=true
    // Generate setup token (24h expiry, SHA-256 hashed in DB)
    // Return user_id
}
```

- [ ] **Step 7: Add invite endpoint + email**

In `api/users.rs` (from PR 2): `POST /api/users/invite` — takes `{ email, role }`, calls `invite_user`. **Note:** actual email sending is not available until PR 5 (ACS). For now, log the setup URL to stderr (same pattern as current password-reset fallback). PR 5 will wire up ACS. Returns 409 if email already exists.

- [ ] **Step 8: Test + commit PR 4**

Test: SSO creates pending user → pending page shown → admin approves → next poll redirects to dashboard. Test invite flow. Test that pending users can't access API endpoints.

```bash
git checkout -b feat/v014-pending-approval
git add -A && git commit -m "feat(v0.14): pending approval flow + SSO complete + invite"
```

---

## Task 5: Azure Communication Services Email (PR 5)

**Files:**
- Modify: `crates/networker-dashboard/Cargo.toml`
- Modify: `crates/networker-dashboard/src/api/auth.rs`
- Modify: `crates/networker-dashboard/src/config.rs`
- Create: `crates/networker-dashboard/src/email.rs`
- Modify: `crates/networker-dashboard/src/main.rs`

- [ ] **Step 1: Add ACS crate, remove lettre**

In `Cargo.toml`: remove `lettre`, add `azure_communication_email` (or use `reqwest` with HMAC if no good crate exists — check crates.io first). **Keep `rand`** — it's used for SSO one-time codes and reset tokens throughout the codebase.

Add `DASHBOARD_ACS_CONNECTION_STRING` and `DASHBOARD_ACS_SENDER` to config.rs.

- [ ] **Step 2: Create email.rs module**

```rust
pub async fn send_email(to: &str, subject: &str, body: &str) -> anyhow::Result<()> {
    // If ACS configured: use ACS client
    // If not: log the email content (fallback for dev/testing)
}

pub fn is_configured() -> bool {
    std::env::var("DASHBOARD_ACS_CONNECTION_STRING").is_ok()
}
```

- [ ] **Step 3: Replace send_reset_email in api/auth.rs**

Replace the `lettre`-based `send_reset_email` with `email::send_email`. Remove all `lettre` imports.

- [ ] **Step 4: Update password reset token storage**

Ensure reset tokens are stored as `SHA-256(token)` in DB. Update `create_reset_token` to hash before storing, and `reset_password_with_token` to hash the submitted token before comparing.

- [ ] **Step 5: Add email module to main.rs**

Add `mod email;` to `main.rs`.

- [ ] **Step 6: Test + commit PR 5**

Test: password reset with ACS (if connection string set). Test fallback logging if not configured.

```bash
git checkout -b feat/v014-acs-email
git add -A && git commit -m "feat(v0.14): Azure Communication Services email, remove lettre"
```

---

## Task 6: Remove Local Tester Auto-Spawn (PR 6)

**Files:**
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `dashboard/src/pages/JobsPage.tsx`

- [ ] **Step 1: Remove auto-spawn from main.rs**

Delete the block from `let local_tester_api_key = {` through `let _ = local_tester_api_key;` (~150 lines). This removes:
- Finding/creating local-tester DB record
- Killing orphaned processes
- Spawning subprocess
- Monitor loop

Keep the `tester_processes` field in `AppState` (still needed for user-created local testers via the Add Tester flow).

- [ ] **Step 2: Add onboarding prompt to JobsPage**

When `testers.length === 0`, show a prominent prompt above the tests list:

```tsx
<div className="border border-cyan-500/20 rounded p-4 mb-4 text-center">
  <p className="text-gray-400 text-sm mb-2">No testers connected</p>
  <p className="text-gray-600 text-xs mb-3">Add a tester to start running network diagnostics</p>
  <button onClick={() => setShowAddTester(true)} className="...">
    Add Tester
  </button>
</div>
```

- [ ] **Step 3: Test + commit PR 6**

Verify: dashboard starts without spawning processes. Verify: "Add Tester" flow still works (local/SSH/endpoint).

```bash
git checkout -b feat/v014-remove-auto-tester
git add -A && git commit -m "feat(v0.14): remove local tester auto-spawn, add onboarding prompt"
```

---

## Task 7: Config Error Page + CLI Setup + Version Bump (PR 7)

**Note:** `az vm run-command` integration (replacing SSH) is deferred to a follow-up PR after v0.14.0 ships. The current SSH-based deployment continues to work.

**Files:**
- Modify: `crates/networker-dashboard/src/main.rs`
- Modify: `crates/networker-dashboard/src/config.rs`
- Create: `dashboard/src/pages/ConfigErrorPage.tsx`
- Modify: `dashboard/src/App.tsx`

- [ ] **Step 1: Add `setup` CLI subcommand**

In `main.rs`, before the server startup, check for `setup` arg:

```rust
let args: Vec<String> = std::env::args().collect();
if args.get(1).map(|s| s.as_str()) == Some("setup") {
    // Interactive: prompt for admin email + password
    // Connect to DB, run migrations, create admin
    // Exit
    return Ok(());
}
```

- [ ] **Step 2: Add startup validation**

After loading config, before connecting to DB: if `admin_email` is None and no users exist in DB, serve an error page instead of the dashboard.

The error page is a static HTML response on all routes:

```rust
if needs_setup {
    let error_html = include_str!("../../../dashboard/src/pages/config-error.html");
    // Serve this on all routes
}
```

Or: set a flag in `AppState` and have the frontend show `ConfigErrorPage` when it gets a specific error from the API.

- [ ] **Step 3: Create ConfigErrorPage.tsx**

Terminal-style error display:
```
╔════════════════════════════════════╗
║  Networker Setup Required          ║
╠════════════════════════════════════╣
║                                    ║
║  DASHBOARD_ADMIN_EMAIL is not set  ║
║                                    ║
║  Set this env var and restart:     ║
║  export DASHBOARD_ADMIN_EMAIL=...  ║
║                                    ║
║  Or run:                           ║
║  $ networker-dashboard setup       ║
╚════════════════════════════════════╝
```

- [ ] **Step 4: Version bump to v0.14.0**

Update all 3 locations:
- `Cargo.toml` workspace version: `0.14.0`
- `CHANGELOG.md`: new `## [0.14.0]` section
- `install.sh` + `install.ps1`: `INSTALLER_VERSION="v0.14.0"`

- [ ] **Step 5: Test + commit PR 7**

Test: missing env var shows error page. Test: `networker-dashboard setup` creates admin. Test: version bump compiles.

```bash
git checkout -b feat/v014-config-error-cli
git add -A && git commit -m "feat(v0.14.0): config error page, CLI setup, version bump"
```

---

## Version Strategy

PRs 1-6 are merged in rapid succession without individual version bumps — they are intermediate steps toward v0.14.0. The version bump is consolidated in PR 7. This is an exception to the "every PR bumps version" rule because these PRs form a single feature release.

## Post-Implementation Checklist

- [ ] All 7 PRs merged to main
- [ ] Tag `v0.14.0` and push (triggers release build)
- [ ] Verify release includes `dashboard-frontend.tar.gz`
- [ ] Deploy to Azure VM via Settings → Update
- [ ] Test SSO login with Microsoft (create app registration in Entra ID)
- [ ] Test SSO login with Google (create OAuth client in Google Cloud Console)
- [ ] Test password reset via ACS email
- [ ] Test pending approval flow end-to-end
- [ ] Update CLAUDE.md with new env vars
- [ ] Update Gist with new installer versions
