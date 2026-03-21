# v0.14 Design Spec — Auth Overhaul: SSO, Email Identity, Role-Based Access

## Overview

Replace username-based authentication with email-based identity. Add Microsoft and Google SSO. Introduce three-tier roles (admin/operator/viewer) with mandatory admin approval for new users. Remove auto-spawned local tester — users add testers manually.

## Decisions Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Login page layout | SSO-first, card container, email "Continue" smart routing | SSO is the primary path; card provides visual containment |
| SSO config | Environment variables | Secrets stay out of DB; set once on deploy |
| Identity field | Email (replaces username) | Universal, works for both SSO and local accounts |
| Admin seed | Env var (`DASHBOARD_ADMIN_EMAIL`), CLI fallback | Automated deploys use env vars; CLI recovers misconfiguration |
| Missing config | Error page with clear message | Better than silent failure or cryptic log |
| Role model | Three-tier: admin / operator / viewer | Covers 90% of use cases without complexity |
| New user default | Pending (no access until admin approves) | Prevents unauthorized access from SSO auto-creation |
| Pending page design | Centered status (icon + message + status card) | Clean, simple, nothing to do but wait |
| Users page design | Unified card list with filter tabs | Mobile-friendly, pending cards have yellow left border |
| Email service | Azure Communication Services | Native Azure, HTTP API, good deliverability |
| Remote commands | `az vm run-command invoke` | No SSH keys, uses Azure RBAC, already available |
| Local tester | Remove auto-spawn | User adds testers manually from UI |
| Multi-project | Deferred to v0.15 | SSO + roles in v0.14 naturally extend to projects later |

## Architecture

### Auth Flow

```
                                    ┌─────────────────────┐
                                    │     Login Page       │
                                    │  ┌───────────────┐   │
                                    │  │ Continue with  │   │
                                    │  │  Microsoft     │───┼──► Microsoft OAuth
                                    │  │ Continue with  │   │        │
                                    │  │  Google        │───┼──► Google OAuth
                                    │  └───────────────┘   │        │
                                    │  ┌───────────────┐   │        │
                                    │  │ Email + Continue│   │        ▼
                                    │  └───────┬───────┘   │   OAuth Callback
                                    └──────────┼───────────┘   /auth/callback/:provider
                                               │                     │
                                               ▼                     ▼
                                    ┌──────────────────┐    ┌──────────────────┐
                                    │ Check email domain│    │ Validate token   │
                                    │ Has SSO config?   │    │ Get user email   │
                                    └────────┬─────────┘    └────────┬─────────┘
                                             │                       │
                                    ┌────────┴────────┐              │
                                    │  SSO domain?    │              │
                                    │  Yes → redirect │              │
                                    │  No → show pwd  │              │
                                    └────────┬────────┘              │
                                             │                       │
                                             ▼                       ▼
                                    ┌──────────────────────────────────┐
                                    │       Find or Create User        │
                                    │  Email exists? → authenticate    │
                                    │  New email? → create as pending  │
                                    └──────────────┬───────────────────┘
                                                   │
                                          ┌────────┴────────┐
                                          │  User status?   │
                                          ├─ pending → Pending Approval Page
                                          ├─ active  → Issue JWT → Dashboard
                                          └─ disabled → "Account disabled"
```

### Database Changes (V008 Migration)

```sql
-- Modify dash_user: drop username, add SSO fields
ALTER TABLE dash_user DROP COLUMN IF EXISTS username;
ALTER TABLE dash_user ALTER COLUMN email SET NOT NULL;
ALTER TABLE dash_user ADD CONSTRAINT dash_user_email_unique UNIQUE (email);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'pending';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS auth_provider VARCHAR(20) NOT NULL DEFAULT 'local';
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_subject_id VARCHAR(255);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS display_name VARCHAR(200);
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS avatar_url TEXT;
ALTER TABLE dash_user ADD COLUMN IF NOT EXISTS sso_only BOOLEAN NOT NULL DEFAULT FALSE;

-- Allow NULL password_hash for SSO-only accounts
ALTER TABLE dash_user ALTER COLUMN password_hash DROP NOT NULL;

-- Migrate existing users: preserve disabled state, activate non-disabled users
UPDATE dash_user SET status = 'active' WHERE disabled = FALSE;
UPDATE dash_user SET status = 'disabled' WHERE disabled = TRUE;
ALTER TABLE dash_user DROP COLUMN IF EXISTS disabled;

-- Hash password reset tokens (store SHA-256 instead of plaintext)
-- Existing tokens are invalidated by this migration (acceptable)

-- Index for SSO lookups
CREATE INDEX IF NOT EXISTS ix_user_sso ON dash_user (auth_provider, sso_subject_id) WHERE sso_subject_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ix_user_status ON dash_user (status);
```

**User status values:** `pending`, `active`, `disabled`

**Auth provider values:** `local`, `microsoft`, `google`

**password_hash:** NULL for SSO-only accounts. When NULL, password login is rejected; user must use their SSO provider.

**password_reset_token:** Stored as SHA-256 hash, not plaintext. Raw token sent in email, hashed on verification. Existing tokens invalidated by migration.

### JWT Claims (Updated)

```rust
struct Claims {
    sub: Uuid,          // user_id
    email: String,      // replaces username
    role: String,       // admin | operator | viewer
    exp: i64,
    iat: i64,
}
```

### API Changes

#### New Public Endpoints (no auth)

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/auth/sso/:provider` | Redirect to SSO provider (Microsoft/Google) |
| GET | `/auth/callback/:provider` | OAuth callback — exchange code, find/create user, issue JWT |
| POST | `/auth/login` | Email + password login (updated: email replaces username) |
| POST | `/auth/forgot-password` | Request password reset email |
| POST | `/auth/reset-password` | Reset password with token |
| GET | `/auth/providers` | List configured SSO providers (no secrets exposed) → `{ microsoft: true, google: false }` |
| POST | `/auth/exchange-code` | Exchange one-time SSO code for JWT (30s expiry on code) |
| POST | `/auth/check-email` | Check if email **domain** has SSO configured → `{ provider: "microsoft" }` or `{ provider: null }`. Domain-level check only — never reveals if a specific user exists. Always shows password field for non-SSO domains. |

#### New Protected Endpoints (JWT required)

| Method | Path | Role | Purpose |
|--------|------|------|---------|
| GET | `/api/users` | admin | List all users |
| GET | `/api/users/pending` | admin | List pending users |
| POST | `/api/users/:id/approve` | admin | Approve user + assign role |
| POST | `/api/users/:id/deny` | admin | Deny pending user |
| PUT | `/api/users/:id/role` | admin | Change user role |
| POST | `/api/users/:id/disable` | admin | Disable user account |
| POST | `/api/users/invite` | admin | Send invite email to new user |
| GET | `/api/auth/profile` | any | Get current user profile |

**Invite flow (`POST /api/users/invite`):**
- Request body: `{ email: string, role: string }`
- Creates `dash_user` with `status = 'pending'`, assigned `role`, `must_change_password = true`
- Generates a one-time setup token (like password reset, 24h expiry)
- Sends email via ACS with context-aware message:
  - If SSO is configured for the email's domain: "You've been invited to Networker Dashboard. Sign in with your {Microsoft/Google} account, or click here to set a local password."
  - Otherwise: "You've been invited to Networker Dashboard. Click here to set your password."
- Admin then approves the user from the Users page (or: admin can pre-approve by setting `auto_approve_invited = true` on the invite, which creates with `status = 'active'`)
- If the email is already registered: returns 409 Conflict

#### Modified Endpoints

| Endpoint | Change |
|----------|--------|
| `POST /auth/login` | Accept `email` instead of `username` |
| `POST /auth/change-password` | Accept optional `email` field (update recovery email) |
| All protected endpoints | Middleware checks `status = 'active'` in addition to JWT validity |

### Role Enforcement Middleware

```rust
// In require_auth middleware, after JWT validation:
// 1. Check user status is 'active' (not pending/disabled)
// 2. Check user role meets endpoint requirement
// 3. Return 403 with clear message if insufficient

fn require_role(min_role: &str) -> impl Fn(Request) -> Result<Request, StatusCode> {
    // admin > operator > viewer
    // "operator" allows admin and operator
    // "viewer" allows all active users
}
```

### SSO Configuration (Environment Variables)

```bash
# Microsoft (Entra ID / Azure AD)
DASHBOARD_MICROSOFT_CLIENT_ID=<app-registration-client-id>
DASHBOARD_MICROSOFT_CLIENT_SECRET=<client-secret>
DASHBOARD_MICROSOFT_TENANT_ID=<tenant-id-or-common>

# Google
DASHBOARD_GOOGLE_CLIENT_ID=<oauth-client-id>
DASHBOARD_GOOGLE_CLIENT_SECRET=<client-secret>

# Email (Azure Communication Services)
DASHBOARD_ACS_CONNECTION_STRING=<connection-string>
DASHBOARD_ACS_SENDER=DoNotReply@<your-domain>.azurecomm.net

# Dashboard public URL (for OAuth callbacks and email links)
DASHBOARD_PUBLIC_URL=https://networker-dash.eastus.cloudapp.azure.com

# Admin seed
DASHBOARD_ADMIN_EMAIL=admin@company.com
DASHBOARD_ADMIN_PASSWORD=<temp-password>  # optional, generated if absent
```

### OAuth Flow (Microsoft Example)

1. **Frontend**: User clicks "Continue with Microsoft"
2. **Frontend**: Redirects to `/auth/sso/microsoft`
3. **Backend**: Constructs Microsoft OAuth URL with `client_id`, `redirect_uri`, `scope=openid email profile`, `state=<random>`
4. **Backend**: Redirects browser to `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize?...`
5. **Microsoft**: User authenticates, consents
6. **Microsoft**: Redirects to `/auth/callback/microsoft?code=<code>&state=<state>`
7. **Backend**: Exchanges code for tokens via `POST /oauth2/v2.0/token`
8. **Backend**: Extracts email + subject_id from ID token
9. **Backend**: Finds or creates `dash_user` with `auth_provider='microsoft'`, `sso_subject_id=<sub>`
10. **Backend**: If new user → status='pending'. If existing active user → proceed.
11. **Backend**: If SSO email matches existing local account → requires password verification to link (see Security Measures).
12. **Backend**: Stores a one-time code in DB (32-byte random, 30s expiry). Redirects to `/auth/sso-complete?code=<code>`
13. **Frontend**: Calls `POST /auth/exchange-code` with the code → receives JWT + user status
14. **Frontend**: Stores JWT in localStorage, navigates to `/` (active) or `/pending` (pending)

### Security Measures

**OAuth CSRF protection:** The `state` parameter is a random 32-byte hex string, stored in an HttpOnly cookie with `SameSite=Lax` and 5-minute TTL. The callback handler validates that the `state` query parameter matches the cookie value before exchanging the authorization code. This avoids server-side session storage.

**JWT delivery after SSO:** Instead of putting the JWT in a URL fragment (`#token=...`), use a one-time authorization code flow:
1. Callback stores a random 32-byte code in DB (valid 30 seconds)
2. Redirects to `/auth/sso-complete?code=<code>`
3. Frontend exchanges code for JWT via `POST /auth/exchange-code`
4. JWT never appears in URLs, browser history, or extension-visible state

**Account linking (SSO + local with same email):** When an SSO login returns an email that already exists as a local account:
- If the existing account has `auth_provider = 'local'`: the user must verify ownership by entering their local password. On success, `auth_provider` is upgraded to the SSO provider and `sso_subject_id` is set. Password is retained as a fallback.
- If the existing account already has a different SSO provider: reject with "This email is already linked to another provider. Contact your admin."

**SSO-only enforcement:** Admin can toggle per-user or per-domain to force SSO-only login. When `sso_only = true`, password login is rejected even if `password_hash` is set. Implemented as a column on `dash_user` (per-user) and optionally as a domain-level rule in config. This prevents SSO users from weakening security by setting a weak local password.

**Password reset tokens:** Stored as `SHA-256(token)` in the database, not plaintext. The raw token is sent in the email. On reset, the submitted token is hashed and compared. This follows the same pattern as Django/Laravel.

**Status enforcement:** Every authenticated request checks `dash_user.status = 'active'` via a DB query (consistent with the existing `must_change_password` enforcement pattern). When an admin disables a user, the next request from that user is rejected. The existing `must_change_password` flow is retained for local accounts only — SSO accounts bypass it permanently (they never see the change-password screen since their auth is delegated to the SSO provider).

**Rate limiting:** Rate-limited endpoints (10 requests/min per IP, via `tower-governor`):
- `POST /auth/login` — brute-force prevention
- `POST /auth/forgot-password` — abuse prevention
- `POST /auth/check-email` — enumeration prevention
- `GET /auth/sso/:provider` — SSO phishing amplification prevention
- `GET /auth/callback/:provider` — replay/abuse prevention (OAuth providers also limit this)

**Domain SSO disclosure:** The `/auth/check-email` endpoint reveals which domains have SSO configured. This is standard practice (Slack, Notion, Microsoft 365 all do this) but can be toggled off via `DASHBOARD_HIDE_SSO_DOMAINS=true` env var. When hidden, the email "Continue" button always shows the password field, and SSO is only available via the explicit provider buttons.

**`az vm run-command` security posture:**
- The identity running `az` commands must have `Virtual Machine Contributor` role (or a least-privilege custom role) at the resource group scope.
- **Use managed identity** (system-assigned on the dashboard VM) instead of user principal with `az login`. This avoids storing credentials and provides automatic rotation.
- **Command audit**: Every invoked command + output (stdout/stderr) is logged to the dashboard's `deployment.log` column AND to Azure Activity Log (automatic with `az vm run-command`).
- **Risk**: Anyone who compromises the dashboard backend gets root on target VMs (run-command executes as root by default). Mitigate with least-privilege managed identity scoped to `networker-rg` only.
- **v0.15 consideration**: Add per-tester approval step for destructive commands (delete, stop).

**Role as Rust enum:** The `role` field in JWT claims and DB should use a Rust enum (`Admin`, `Operator`, `Viewer`) to prevent typos and enable compile-time checking. Serialized as lowercase strings in JSON/DB for backward compatibility.

### Frontend Changes

#### New Pages

| Page | Route | Access |
|------|-------|--------|
| Login (redesigned) | `/login` | Public |
| Forgot Password | `/forgot-password` | Public |
| Reset Password | `/reset-password?token=...` | Public |
| SSO Complete | `/auth/sso-complete` | Public (exchanges one-time code for JWT) |
| Pending Approval | `/pending` | Authenticated, status=pending |
| Users Management | `/users` | admin only |

#### Login Page (Card Container — Option B)

- "Continue with Microsoft" button (shown if `DASHBOARD_MICROSOFT_CLIENT_ID` is configured)
- "Continue with Google" button (shown if `DASHBOARD_GOOGLE_CLIENT_ID` is configured)
- "or" divider
- Email input + "Continue" button
  - On Continue: calls `POST /auth/check-email` → if SSO domain, redirects to provider. Otherwise shows password field.
- "Forgot password?" link
- SSO button visibility: frontend calls a new `GET /auth/providers` endpoint on page load to know which SSO providers are configured

#### Pending Approval Page (Centered Status — Option A)

- Networker branding at top
- ⏳ icon in a circle with yellow border
- "Account pending approval" heading
- "Signed in as user@email.com" in green
- Status card: "Waiting for admin approval"
- "Sign out" button
- Auto-polls `/auth/profile` every 10s — if status becomes 'active', redirects to `/`. (v0.15: replace polling with WebSocket/SSE for instant approval notification)

#### Users Management Page (Unified Card List — Option B)

- Header: "Users" + filter tabs ("Pending (N)" / "All (N)") + "Invite" button
- Pending users: cards with yellow left border, inline role selector + "Approve" button + deny "✕"
- Active users: cards with role badge (admin=green, operator=cyan, viewer=gray)
- Each card shows: email, auth provider, last login / request time
- Click card to expand: change role, disable account
- Admin-only sidebar entry (hidden for operator/viewer)

#### Sidebar Changes

- Add "Users" entry (visible only to admin role, with pending count badge)
- All existing entries remain

#### Auth Store Changes

```typescript
interface AuthState {
  token: string | null;
  email: string;        // replaces username
  role: string;
  status: string;       // 'active' | 'pending' | 'disabled'
  isAuthenticated: boolean;
  login: (token, email, role, status) => void;
  logout: () => void;
}
```

### Startup Flow Changes

```
1. Load config
2. Validate required env vars:
   - DASHBOARD_JWT_SECRET (required)
   - DASHBOARD_DB_URL (required)
   - DASHBOARD_ADMIN_EMAIL (required — error page if missing)
3. Connect to PostgreSQL
4. Run migrations (V002-V008)
5. Seed admin:
   - If no users exist:
     - Create admin with DASHBOARD_ADMIN_EMAIL
     - Password: DASHBOARD_ADMIN_PASSWORD or random temp (printed to stderr)
     - status='active', role='admin', must_change_password=true
6. Start scheduler background task
7. Start HTTP server
   - NO local tester auto-spawn
```

**Missing Config Error Page:**
If `DASHBOARD_ADMIN_EMAIL` is not set and no users exist in DB, the dashboard serves a static error page on all routes:

```
╔══════════════════════════════════════╗
║  Networker Dashboard Setup Required  ║
╠══════════════════════════════════════╣
║                                      ║
║  Missing required configuration:     ║
║                                      ║
║  DASHBOARD_ADMIN_EMAIL               ║
║    Set this to create the admin      ║
║    account on first startup.         ║
║                                      ║
║  Or run:                             ║
║  $ networker-dashboard setup         ║
║                                      ║
╚══════════════════════════════════════╝
```

**CLI Setup Command:**
```bash
$ DASHBOARD_DB_URL="postgres://..." networker-dashboard setup
Admin email: admin@company.com
Admin password: ********
Confirm password: ********
Admin user created. Start the dashboard with:
  DASHBOARD_JWT_SECRET=<secret> DASHBOARD_ADMIN_EMAIL=admin@company.com networker-dashboard
```
Requires `DASHBOARD_DB_URL` environment variable (reads from env, not CLI flags). Runs migrations before creating the user.

### Tester Management (No Auto-Spawn)

The local tester auto-spawn logic in `main.rs` (lines 142-287) is removed entirely. Instead:

- Tests page shows "No testers online" with a prompt to add one
- "Add Tester" flow remains as-is (local / SSH / on deployed endpoint)
- The monitor loop that respawns dead testers is kept for user-created local testers

### Email via Azure Communication Services

Replace the `lettre` SMTP integration with Azure Communication Services. Use the `azure_communication_email` community crate (or `azure-communications`) to avoid manually implementing HMAC-SHA256 request signing, which is tedious and error-prone (wrong timestamp/nonce → 401s that are hard to debug).

```rust
// Cargo.toml
azure_communication_email = "0.1"  // or equivalent community crate

async fn send_email(to: &str, subject: &str, body: &str) -> anyhow::Result<()> {
    let conn_str = std::env::var("DASHBOARD_ACS_CONNECTION_STRING")?;
    let sender = std::env::var("DASHBOARD_ACS_SENDER")?;
    // Use crate's client, not manual HMAC signing
}
```

Falls back to logging the reset URL if ACS is not configured (same behavior as current SMTP fallback).

**Deployment checklist for ACS email:**
1. Create Azure Communication Services resource in Azure portal
2. Configure email domain: either Azure-managed (`*.azurecomm.net`) or custom domain
3. **For custom domains**: set up SPF, DKIM, and DMARC records in DNS — without these, deliverability collapses quickly (emails go to spam or get rejected)
4. Set `DASHBOARD_ACS_CONNECTION_STRING` and `DASHBOARD_ACS_SENDER` env vars
5. Verify sender address in Azure portal before first send

### Remote Commands via Azure VM Run Command

For endpoint updates and tester deployment, replace SSH with:

```rust
async fn run_remote_command(vm_name: &str, resource_group: &str, script: &str) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("az")
        .args([
            "vm", "run-command", "invoke",
            "--resource-group", resource_group,
            "--name", vm_name,
            "--command-id", "RunShellScript",
            "--scripts", script,
        ])
        .output()
        .await?;
    // Parse JSON output for stdout/stderr
}
```

## Implementation Order (PRs)

1. **PR 1: DB migration + email identity** — V008 migration, replace username with email in db/users.rs, api/auth.rs, JWT claims, frontend auth store. Admin seed with email env var. CLI setup command.
2. **PR 2: Role enforcement** — Three-tier roles in middleware, role-based route protection, Users management page (admin only).
3. **PR 3: SSO integration** — Microsoft + Google OAuth flows, callback handling, `check-email` endpoint, frontend SSO buttons + redirect.
4. **PR 4: Pending approval flow** — Pending status enforcement in middleware, pending approval page, admin approve/deny in Users page, auto-poll for status change.
5. **PR 5: Azure Communication Services email** — Replace lettre/SMTP with ACS HTTP API, password reset via ACS. Remove `lettre` and `rand` crate dependencies from Cargo.toml.
6. **PR 6: Remove local tester auto-spawn** — Remove auto-spawn from main.rs, add onboarding prompt on Tests page.
7. **PR 7: Missing config error page + az vm run-command** — Error page for missing env vars, replace SSH with az vm run-command.

## Version Bump

v0.13.30 → v0.14.0

All 3 sync locations: Cargo.toml workspace version, CHANGELOG.md, install.sh/install.ps1 INSTALLER_VERSION.

## Out of Scope (v0.15)

- Multi-project / multi-tenancy
- Project-scoped cloud provider accounts
- Role-based VM creation permissions
- Granular test visibility rules
- Public share links with expiration
