# Dashboard Guide

This guide covers the current `networker-dashboard` control-plane workflow: first boot,
authentication, tester lifecycle, and the main UI areas.

## What The Dashboard Does

The dashboard is the browser-based control plane for:

- managing testers (`networker-agent` workers)
- creating and monitoring tests
- viewing historical runs and rich run details
- deploying endpoints and testers
- scheduling recurring tests
- managing users, roles, and approvals
- checking versions and running self-update flows

The frontend lives in [`dashboard/`](../dashboard/) and the backend lives in
[`crates/networker-dashboard`](../crates/networker-dashboard/).

## First Boot

The dashboard uses environment variables rather than a JSON config file.

### Required

| Variable | Purpose | Default |
|---|---|---|
| `DASHBOARD_JWT_SECRET` | JWT signing secret | none, must be set |

### Common

| Variable | Purpose | Default |
|---|---|---|
| `DASHBOARD_DB_URL` | PostgreSQL connection string | `postgres://networker:networker@localhost:5432/networker_dashboard` |
| `DASHBOARD_ADMIN_EMAIL` | Initial admin seed identity | unset |
| `DASHBOARD_ADMIN_PASSWORD` | Initial admin password | prompt on TTY, or generated temp password non-interactively |
| `DASHBOARD_PORT` | HTTP port | `3000` |
| `DASHBOARD_BIND_ADDR` | Bind address | `127.0.0.1` |
| `DASHBOARD_CORS_ORIGIN` | Allowed frontend origin | unset |
| `DASHBOARD_PUBLIC_URL` | External base URL used for links and callbacks | `http://localhost:<port>` |
| `DASHBOARD_STATIC_DIR` | Built frontend directory | `./dashboard/dist` |

### Optional SSO

| Variable | Purpose |
|---|---|
| `SSO_MICROSOFT_CLIENT_ID` | Microsoft Entra ID client ID |
| `SSO_MICROSOFT_CLIENT_SECRET` | Microsoft Entra ID client secret |
| `SSO_MICROSOFT_TENANT_ID` | Tenant ID, defaults to `common` |
| `SSO_GOOGLE_CLIENT_ID` | Google OAuth client ID |
| `SSO_GOOGLE_CLIENT_SECRET` | Google OAuth client secret |

### Optional Email Delivery

| Variable | Purpose |
|---|---|
| `DASHBOARD_ACS_CONNECTION_STRING` | Azure Communication Services connection string |
| `DASHBOARD_ACS_SENDER` | Verified ACS sender address |

## Setup Options

### Option 1: Seed The Admin From Environment

Use this for automated or repeatable deployments:

```bash
export DASHBOARD_JWT_SECRET="$(openssl rand -base64 32)"
export DASHBOARD_ADMIN_EMAIL="admin@example.com"
export DASHBOARD_ADMIN_PASSWORD="ChangeMe123!"
export DASHBOARD_STATIC_DIR="./dashboard/dist"
cargo run -p networker-dashboard
```

On startup the dashboard runs migrations, checks whether users exist, and seeds the admin user
when `DASHBOARD_ADMIN_EMAIL` is set.

### Option 2: Interactive Setup Command

Use this when no users exist and you want to create the first admin interactively:

```bash
export DASHBOARD_DB_URL="postgres://networker:networker@localhost:5432/networker_dashboard"
cargo run -p networker-dashboard -- setup
```

The `setup` command prompts for:

- admin email
- admin password
- password confirmation

### Setup-Required Error Page

If no users exist and `DASHBOARD_ADMIN_EMAIL` is not set, the dashboard does not start normally.
Instead it serves a setup-required page telling you to either:

- set `DASHBOARD_ADMIN_EMAIL`, or
- run `networker-dashboard setup`

## Local Development Flow

```bash
docker compose -f docker-compose.db.yml up -d
cargo build --release
cd dashboard && npm install && npm run build && cd ..
export DASHBOARD_JWT_SECRET="$(openssl rand -base64 32)"
export DASHBOARD_ADMIN_EMAIL="admin@example.com"
export DASHBOARD_ADMIN_PASSWORD="ChangeMe123!"
export DASHBOARD_STATIC_DIR="./dashboard/dist"
cargo run -p networker-dashboard
```

Open `http://localhost:3000`.

## Authentication And User Model

The current dashboard auth model is email-based.

- login uses email and password
- forgot-password and reset-password flows are available
- users have roles: `admin`, `operator`, `viewer`
- new invited or SSO-created users can be `pending` until approved
- pending users are restricted to the pending and password-change flows
- the Users page is admin-only

### Current UI Behavior

- The login page is email/password-first today.
- The app includes an `SSOCompletePage` and backend SSO endpoints for Microsoft and Google callback
  and exchange flows.
- The current frontend does not render provider buttons on the login page yet, so do not document
  SSO as the primary day-one sign-in path for operators unless you are adding that UI too.
- `DASHBOARD_PUBLIC_URL` matters for password-reset links and SSO callback URLs.

## Tester Lifecycle

The dashboard no longer auto-spawns a local tester on startup.

Instead, users add testers manually from the Tests page:

- `local`: spawn a tester on the dashboard machine
- `ssh`: provision a tester over SSH
- `endpoint`: install a tester onto an existing deployed endpoint machine

This matches the current backend behavior in [`api/agents.rs`](../crates/networker-dashboard/src/api/agents.rs)
and the Tests UI in [`JobsPage.tsx`](../dashboard/src/pages/JobsPage.tsx).

## Main UI Areas

| Area | Purpose |
|---|---|
| Dashboard | high-level system summary |
| Deploy | create and inspect deployments |
| Tests | create tests, view testers, add testers |
| Schedules | recurring test definitions and next-run timing |
| Runs | historical results and attempt drill-down |
| Settings | cloud status, updates, operational settings |
| Users | admin-only user management, pending approvals, invites, role changes |

## Admin Workflows

### Approve Pending Users

The Users page lets admins:

- review pending users
- approve them with `viewer`, `operator`, or `admin`
- deny pending requests

### Invite Users

Admins can invite users by email and assign a starting role.

### Manage Roles

Admins can:

- change an active user's role
- disable a user

## Related Docs

- [`installation.md`](installation.md)
- [`architecture.md`](architecture.md)
- [`cloud-auth.md`](cloud-auth.md)
- [`testing.md`](testing.md)
