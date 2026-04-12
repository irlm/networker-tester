# Auth/SSO Configuration + User Onboarding Design

> **Status:** Approved  
> **Date:** 2026-04-12  
> **Scope:** Dynamic SSO provider management, batch user import, pending-project acceptance flow

---

## Problem

SSO providers (Microsoft, Google) are configured via environment variables, requiring SSH access to the server. There is no UI for system admins to manage SSO configuration. Additionally, project admins need a way to bulk-import users and track invitation status, and users need a smooth onboarding flow when invited to new projects.

## Goals

1. System admins configure SSO providers from the browser (no env vars, no restart)
2. Support any number of OIDC-compatible providers (Microsoft, Google, Okta, Auth0, etc.)
3. Project admins can batch-import users via CSV and manage invite status
4. Users see pending project invitations on login and can accept/deny/ignore

## Non-Goals

- SAML support (OIDC only for v1)
- Self-service user registration without admin or invite (existing approval flow stays)
- SCIM provisioning

---

## 1. Dynamic SSO Provider Management

### 1.1 Data Model

New table `sso_provider`:

```sql
CREATE TABLE sso_provider (
    provider_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name                 VARCHAR(200) NOT NULL,
    provider_type        VARCHAR(30)  NOT NULL,  -- 'microsoft', 'google', 'oidc_generic'
    client_id            TEXT         NOT NULL,
    client_secret_enc    BYTEA        NOT NULL,  -- AES-256-GCM encrypted
    client_secret_nonce  BYTEA        NOT NULL,
    issuer_url           TEXT,                    -- required for oidc_generic
    tenant_id            TEXT,                    -- microsoft-specific
    enabled              BOOLEAN      NOT NULL DEFAULT TRUE,
    display_order        SMALLINT     NOT NULL DEFAULT 0,
    created_by           UUID         REFERENCES dash_user(user_id),
    created_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);
```

**Provider types and their well-known endpoints:**

| Type | Authorization | Token | Discovery |
|------|--------------|-------|-----------|
| `microsoft` | `https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/authorize` | `https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token` | Derived from tenant_id |
| `google` | `https://accounts.google.com/o/oauth2/v2/auth` | `https://oauth2.googleapis.com/token` | Hardcoded |
| `oidc_generic` | Discovered from `{issuer_url}/.well-known/openid-configuration` | Discovered | Via issuer_url |

**Validation rules:**
- `microsoft` requires `tenant_id` to be non-empty
- `oidc_generic` requires `issuer_url` to be a valid HTTPS URL
- `client_id` and `client_secret` always required
- `name` must be unique across all providers

### 1.2 API Endpoints (Platform Admin only)

```
GET    /api/admin/sso-providers           — list all providers (secrets redacted)
POST   /api/admin/sso-providers           — create provider
PUT    /api/admin/sso-providers/{id}      — update provider (omit client_secret to keep existing)
DELETE /api/admin/sso-providers/{id}      — delete provider
POST   /api/admin/sso-providers/{id}/test — test provider config (initiates test auth flow)
```

All endpoints require `is_platform_admin = true`.

Response shape (secrets never returned):
```json
{
  "provider_id": "uuid",
  "name": "Microsoft Entra ID",
  "provider_type": "microsoft",
  "client_id": "abc-123",
  "has_client_secret": true,
  "issuer_url": null,
  "tenant_id": "contoso.onmicrosoft.com",
  "enabled": true,
  "display_order": 0
}
```

### 1.3 Runtime Behavior

- On startup, the dashboard loads all enabled providers from DB into an in-memory cache in `AppState`
- The cache is refreshed on any CRUD operation (no restart needed)
- `GET /api/auth/sso/providers` reads from the cache and returns enabled providers for the login page
- `GET /api/auth/sso/init?provider={provider_id}` uses the cached config to build the OAuth redirect URL
- The existing SSO callback and exchange flow remain unchanged — they just look up the provider config from cache instead of env vars

**Backwards compatibility:** If `MICROSOFT_CLIENT_ID` env var is set and no `sso_provider` rows exist for type `microsoft`, the dashboard auto-creates a provider row on startup (one-time migration from env vars to DB). Same for Google. This ensures existing deployments don't break.

### 1.4 Frontend — System Admin > Auth Tab

New tab on `/admin/system`: **"Auth"**

Content:
- **Public URL** field (used for OAuth redirect URIs; currently `DASHBOARD_PUBLIC_URL` env var, now stored in `system_config`)
- **SSO Providers** section:
  - Table listing configured providers: name, type, enabled status, actions (edit/delete)
  - "+ Add Provider" button opens a form:
    - Provider type dropdown (Microsoft Entra ID / Google / Generic OIDC)
    - Name (auto-filled based on type, editable)
    - Client ID (text)
    - Client Secret (password field)
    - Tenant ID (shown for Microsoft)
    - Issuer URL (shown for Generic OIDC)
    - Enabled toggle
  - Edit opens the same form pre-filled (client secret shown as "••••••••", submit without changing keeps existing)
  - Drag handle or order field for display_order (controls button order on login page)

### 1.5 System Config Table

For non-secret platform-level settings (like public URL) that shouldn't be env vars:

```sql
CREATE TABLE system_config (
    key         VARCHAR(100) PRIMARY KEY,
    value       TEXT NOT NULL,
    updated_by  UUID REFERENCES dash_user(user_id),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

Initial keys: `public_url`. Plain text — no encryption needed for non-secrets.

---

## 2. Batch CSV User Import

### 2.1 CSV Format

```csv
email,role
john@company.com,operator
jane@company.com,viewer
team-lead@company.com,admin
```

Two columns: `email` (required), `role` (required, one of: `admin`, `operator`, `viewer`).

### 2.2 Import Logic (per row)

```
For each (email, role) in CSV:
  1. Look up user by email
     - Not found → create placeholder user:
         status = 'active'  (no approval needed — admin explicitly imported them)
         password_hash = NULL  (can't login with password until set via SSO or password reset)
         auth_provider = NULL  (determined on first login — SSO or local)
         must_change_password = FALSE  (SSO users never need this; local users set password via reset flow)
     - Found → use existing user

  2. Check project_member for (project_id, user_id)
     - Already exists with status = 'active' → skip, report as "already member"
     - Already exists with status = 'pending_acceptance' → skip, report as "already invited"
     - Already exists with status = 'denied' → reset to 'pending_acceptance', report as "re-invited"
     - Not found → insert with status = 'pending_acceptance'

  3. Record result for the import summary
```

### 2.3 Import API

```
POST /api/projects/{pid}/members/import
Content-Type: multipart/form-data
Body: file=<CSV file>

Response:
{
  "imported": 5,
  "skipped": 2,
  "errors": 0,
  "details": [
    {"email": "john@co.com", "status": "invited", "message": "New user created + invited as operator"},
    {"email": "jane@co.com", "status": "already_member", "message": "Already a viewer in this project"},
    {"email": "bad-email", "status": "error", "message": "Invalid email format"}
  ]
}
```

Requires project role `admin`.

### 2.4 Invite Email Management

The Members page gains:
- **Status column** showing: `active` (green), `pending` (yellow), `denied` (red)
- **Filter dropdown**: All / Active / Pending / Denied
- **Checkboxes** on pending rows for bulk selection
- **"Send Invite Email"** button (for selected pending members):
  - Generates an invite token (existing `workspace_invite` flow)
  - Sends email with a link to accept
  - Updates the member row with `invite_sent_at` timestamp
- **"Resend"** action per row (for pending members who were already sent an email)

### 2.5 Project Member Status

Add `status` column to `project_member`:

```sql
ALTER TABLE project_member
  ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'active',
  ADD COLUMN IF NOT EXISTS invite_sent_at TIMESTAMPTZ;
```

Values: `'active'` (full member), `'pending_acceptance'` (imported but not yet accepted), `'denied'` (user declined).

Migration backfills all existing rows with `status = 'active'`.

---

## 3. Login Pending-Project Prompt

### 3.1 Flow

After successful login (password or SSO):

1. Backend checks: `SELECT * FROM project_member WHERE user_id = $1 AND status = 'pending_acceptance'`
2. If any pending memberships exist, JWT includes a `has_pending_projects: true` flag
3. Frontend checks this flag and shows a modal before routing to the default project

### 3.2 Pending Projects Modal

Content:
```
You have pending project invitations:

┌─────────────────────────────────────────────┐
│  Project X          Role: Operator          │
│  Invited by admin@company.com               │
│                    [Accept] [Deny] [Ignore]  │
├─────────────────────────────────────────────┤
│  Project Y          Role: Viewer            │
│  Invited by lead@company.com                │
│                    [Accept] [Deny] [Ignore]  │
└─────────────────────────────────────────────┘

                                      [Continue]
```

**Actions:**
- **Accept** → `PUT /api/projects/{pid}/members/me/accept` → sets `status = 'active'`
- **Deny** → `PUT /api/projects/{pid}/members/me/deny` → sets `status = 'denied'`
- **Ignore** → no API call, just dismisses for this session
- **Continue** → closes modal, proceeds to default project

### 3.3 API Endpoints

```
GET    /api/me/pending-projects                    — list pending project memberships
PUT    /api/projects/{pid}/members/me/accept       — accept pending membership
PUT    /api/projects/{pid}/members/me/deny         — deny pending membership
```

These endpoints require only a valid JWT (no project role needed — the user isn't a member yet).

---

## 4. Migration Plan

Single migration (V030):

```sql
-- 1. SSO providers table
CREATE TABLE IF NOT EXISTS sso_provider ( ... );

-- 2. System config table  
CREATE TABLE IF NOT EXISTS system_config ( ... );

-- 3. Project member status + invite tracking
ALTER TABLE project_member
  ADD COLUMN IF NOT EXISTS status VARCHAR(20) NOT NULL DEFAULT 'active',
  ADD COLUMN IF NOT EXISTS invite_sent_at TIMESTAMPTZ;
```

---

## 5. Security Considerations

- SSO client secrets are encrypted at rest with AES-256-GCM using the existing `DASHBOARD_CREDENTIAL_KEY`
- Client secrets are never returned in API responses (`has_client_secret: true` instead)
- The SSO admin endpoints require `is_platform_admin`
- The import endpoint requires project `admin` role
- Pending membership accept/deny only works for the authenticated user's own memberships
- Generic OIDC issuer URLs must be HTTPS
- ID token issuer and audience are validated against the stored provider config

## 6. Frontend Changes Summary

| Page | Change |
|------|--------|
| `/admin/system` | New "Auth" tab with SSO provider CRUD + public URL config |
| `/login` | Render buttons dynamically from `/api/auth/sso/providers` (already does this) |
| `/projects/{pid}/members` | Add status column, filter, CSV import button, bulk send/resend invite |
| Post-login | New pending-projects modal (shown when `has_pending_projects` is true) |
