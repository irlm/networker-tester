# Database Separation & Disaster Recovery Design (Revised)

**Date:** 2026-04-07
**Status:** Approved (with implementation clarifications)
**Scope:** Split single PostgreSQL into core + logs databases on a dedicated DB server, add backups to Azure Blob, recovery CLI, and sandbox provisioning.

---

## 1. Architecture Overview

Two Azure VMs, two PostgreSQL databases:

| VM             | Role          | Size          | Runs                                                                           |
| -------------- | ------------- | ------------- | ------------------------------------------------------------------------------ |
| **db-server**  | Database only | B1ls (~$4/mo) | PostgreSQL 16 with `networker_core` + `networker_logs`, WAL archiving, backups |
| **web-server** | Application   | B1ls          | Dashboard, endpoint, agent, frontend                                           |

- Both VMs are in the same VNet/subnet
- PostgreSQL listens on private IP only
- No public exposure for database

---

## 2. Database Split

| Database         | Tables                                   | Retention     | Backed up? |
| ---------------- | ---------------------------------------- | ------------- | ---------- |
| `networker_core` | All current tables except logs           | Permanent     | Yes        |
| `networker_logs` | `benchmark_request_progress`, `perf_log` | 7-day rolling | No         |

### Migration approach (authoritative)

Migration is handled **entirely outside the application** via a one-time script (`scripts/migrate-to-split.sh`).

Steps:

1. Stop dashboard
2. Create `networker_core`
3. Copy schema + data from `networker_dashboard`
4. Create `networker_logs`
5. Move log tables (`benchmark_request_progress`, `perf_log`)
6. Drop log tables from core
7. Update connection string
8. Restart dashboard
9. Drop old database

**Important:**

- Application migrations (e.g., V026) MUST NOT create or move databases
- Application migrations only manage schema inside `networker_core`

Estimated downtime: ~5 minutes (current scale)

---

## 3. Connection Pool Architecture

```
DashboardState {
    core_pool: Pool,
    logs_pool: Pool,
}
```

### Configuration

| Env var                 | Purpose                                     |
| ----------------------- | ------------------------------------------- |
| `DASHBOARD_DB_URL`      | Core database                               |
| `DASHBOARD_LOGS_DB_URL` | Logs database (recommended explicit config) |

- If not provided, logs URL may be derived (fallback only)

### Pool sizing

- Core: 16
- Logs: 8

### Code changes

Only two modules use logs DB:

- `benchmark_progress.rs`
- `perf_log.rs`

---

## 4. Database Security

### Network

- No public IP on db-server
- NSG allows 5432 only from web-server private IP

### Authentication

| User          | Access                          |
| ------------- | ------------------------------- |
| `app_core`    | CRUD core                       |
| `app_logs`    | CRUD logs                       |
| `backup_user` | Read-only (for logical backups) |
| `dev_admin`   | Read-only                       |
| `dev_write`   | Write via explicit role         |

**Note:** WAL archiving is handled by PostgreSQL server configuration, not user privileges.

### TLS

- `ssl = on`
- Connections use `sslmode=require`

### Dev access

SSH tunnel via web-server.

Recommendation:

- Restrict SSH access to approved IPs or use Azure Bastion

---

## 5. Backup Strategy

### Current model

| Type    | Method        | Retention |
| ------- | ------------- | --------- |
| Daily   | `pg_dump`     | 30 days   |
| Monthly | retained dump | 12 months |

`networker_logs` is not backed up.

### Important clarification (PITR)

This design supports:

- Full restore from logical backups

This design does NOT fully support true point-in-time recovery unless:

- Physical base backups are added (`pg_basebackup`)
- WAL archiving is configured for replay from that base

If PITR is required in future:

- Add physical base backup
- Use WAL replay from that base
- This is listed as an optional future improvement

### Azure Blob lifecycle policy

- Delete daily backups older than 30 days
- Delete monthly backups older than 365 days
- Lifecycle policy enforced by Azure — no application-side cron needed

### Backup verification

After each daily backup:
1. Write `last_backup.json` to Blob with timestamp, size, and checksum
2. Scheduler on web-server reads this marker hourly to verify backup freshness

---

## 6. Logs Retention

Daily cleanup:

```sql
DELETE FROM benchmark_request_progress WHERE created_at < now() - interval '7 days';
DELETE FROM perf_log WHERE logged_at < now() - interval '7 days';
VACUUM ANALYZE benchmark_request_progress;
VACUUM ANALYZE perf_log;
```

If volume grows:

- Switch to batched deletes (e.g., 10k rows per batch with short sleep)

---

## 7. Disaster Recovery CLI (`scripts/infra.sh`)

Single entry point for all infrastructure operations.

### Commands

```bash
# Full production restore
./infra.sh restore --env prod --confirm-production

# Restore just the database server
./infra.sh restore --env prod --db-only --confirm-production

# Create sandbox with anonymized prod data
./infra.sh sandbox --name dev-igor

# Tear down a sandbox
./infra.sh sandbox --name dev-igor --destroy

# List all running sandboxes
./infra.sh list
```

### Full restore flow

Target RTO: ~15 minutes (current data size).

**Note:** Time depends on dataset size and may increase over time.

| Step | Action |
|------|--------|
| 1 | Provision db-server VM from pre-baked Azure image |
| 2 | Download latest backup from Blob |
| 3 | `pg_restore` networker_core |
| 4 | Create empty `networker_logs` with schema |
| 5 | Apply pg_hba.conf, TLS, DB users |
| 6 | Provision web-server VM from pre-baked image (unless `--db-only`) |
| 7 | Deploy binaries from latest GitHub release tag |
| 8 | Configure connection strings to new db-server |
| 9 | Run health checks, print status report |

---

## 8. Sandbox

- Single VM (DB + app together)
- Restore core DB from latest backup
- Anonymize sensitive data
- Logs DB empty (fresh schema only)

### Anonymization

| Table | Field | Action |
|-------|-------|--------|
| `dash_user` | email | `user_{id}@sandbox.local` |
| `dash_user` | display_name | `User {id}` |
| `dash_user` | password_hash | Reset to shared sandbox password |
| `dash_user` | avatar_url | `NULL` |
| `dash_user` | password_reset_token, password_reset_expires | `NULL` |
| `cloud_account` | credentials_enc, credentials_nonce | DELETE all rows |
| `cloud_connection` | * | DELETE all rows |
| `workspace_invite` | * | DELETE all rows |
| `share_link` | * | DELETE all rows |

### Safety

- `--ttl` auto-shutdown (default 48h)
- Sandbox names prefixed `sandbox-` in Azure, tagged `env=sandbox`
- `infra.sh list` shows running sandboxes with age and cost

---

## 9. Monitoring

Health checks run hourly from web-server scheduler:

| Check | Healthy when |
|-------|-------------|
| Core DB connectivity | `SELECT 1` responds < 2s |
| Logs DB connectivity | `SELECT 1` responds < 2s |
| Core DB size | Below 5GB (configurable) |
| Logs DB size | Below 2GB (configurable) |
| Logs retention | Oldest row < 8 days |
| Last backup age | < 25 hours |
| Backup size trend | < 50% growth vs previous |

### Storage

Results written to `system_health` table in `networker_core` (rolling 7 days).

### UI

Settings page gets a **System Health** panel with status indicators and backup info.

### Test restore button

- Admin-only
- Rate-limited
- Single execution at a time

---

## 10. Local Dev

Updated `docker-compose.dashboard.yml` uses single PostgreSQL instance with two databases.

Init script (`scripts/init-logs-db.sql`) creates `networker_logs` database.

**Important:**

- Init scripts run only on fresh volumes
- Existing volumes require manual `CREATE DATABASE networker_logs`

---

## 11. Files to Create/Modify

### New files
- `scripts/infra.sh` — DR CLI (restore, sandbox, list)
- `scripts/migrate-to-split.sh` — One-time database migration
- `scripts/init-logs-db.sql` — Docker entrypoint script for logs DB
- `scripts/anonymize.sql` — Sandbox data anonymization queries
- `scripts/backup-daily.sh` — pg_dump + upload to Blob
- `scripts/retention-cleanup.sh` — 7-day log deletion + vacuum

### Modified files
- `crates/networker-dashboard/src/db/mod.rs` — Add `logs_pool`, `create_logs_pool()`
- `crates/networker-dashboard/src/db/benchmark_progress.rs` — Use `logs_pool`
- `crates/networker-dashboard/src/db/perf_log.rs` — Use `logs_pool`
- `crates/networker-dashboard/src/config.rs` — Add `DASHBOARD_LOGS_DB_URL`
- `crates/networker-dashboard/src/state.rs` — Add `logs_pool` field
- `crates/networker-dashboard/src/scheduler.rs` — Add health check task
- `crates/networker-dashboard/src/api/` — Add `/api/system/health` endpoint
- `docker-compose.dashboard.yml` — Rename DB, add init script
- `dashboard/` — System Health panel on Settings page

**Note:** No in-app migration should create databases.

---

## 12. Future Path (Multi-Region)

- Each datacenter gets its own db-server + web-server pair
- `server_registry.db_url` already stores per-server database URLs
- Replicate `networker_core` across datacenters (PostgreSQL logical replication)
- `networker_logs` stays local per datacenter — never replicated
- Azure Front Door routes to the closest web-server
- `infra.sh restore` gains `--zone` flag to target a specific datacenter

---

## Summary

This design is ready for implementation with clarified responsibilities:

- External script owns database split
- App owns schema only
- Backup model aligned with actual restore capability

Remaining improvements (optional, future):

- Add physical backups (`pg_basebackup`) for true PITR
- Harden SSH access (IP restrictions or Azure Bastion)
- Batched deletes for large-scale log retention
