# Database Separation & Disaster Recovery Design

**Date:** 2026-04-07
**Status:** Approved
**Scope:** Split single PostgreSQL into core + logs databases on a dedicated DB server, add GFS backups to Azure Blob, recovery CLI, and sandbox provisioning.

---

## 1. Architecture Overview

Two Azure VMs, two PostgreSQL databases:

| VM | Role | Size | Runs |
|----|------|------|------|
| **db-server** | Database only | B1ls (~$4/mo) | PostgreSQL 16 with `networker_core` + `networker_logs`, WAL archiving, pg_dump cron |
| **web-server** | Application | B1ls (existing `20.42.8.158`) | Dashboard, endpoint, agent, frontend — connects to db-server over private IP |

Both VMs in the same Azure VNet/subnet. PostgreSQL listens on private IP only (no public exposure).

## 2. Database Split

| Database | Tables | Retention | Backed up? |
|----------|--------|-----------|------------|
| `networker_core` | All current tables except the two below | Permanent | Yes — daily pg_dump + WAL archiving |
| `networker_logs` | `benchmark_request_progress`, `perf_log` | 7-day rolling delete | No — ephemeral by design |

### Migration path

Migration happens outside the application (can't create databases or rename them from within a connection to the target DB). A one-time migration script (`scripts/migrate-to-split.sh`) run during a maintenance window:

1. Stop the dashboard application
2. `CREATE DATABASE networker_core` with schema copied from `networker_dashboard` (via `pg_dump -s | pg_restore`)
3. Copy all data from `networker_dashboard` to `networker_core` (`pg_dump -a | pg_restore`)
4. `CREATE DATABASE networker_logs` with only the two ephemeral table schemas
5. Copy `benchmark_request_progress` and `perf_log` data to `networker_logs`
6. Drop those two tables from `networker_core`
7. Update `DASHBOARD_DB_URL` to point to `networker_core`
8. Restart dashboard — V026 in-app migration adds `system_health` table to `networker_core`
9. Verify, then drop old `networker_dashboard` database

Estimated downtime: ~5 minutes for current data volume.

## 3. Connection Pool Architecture

```
DashboardState {
    core_pool: Pool,    // networker_core — all existing queries
    logs_pool: Pool,    // networker_logs — benchmark_request_progress + perf_log
}
```

### Configuration

| Env var | Default | Purpose |
|---------|---------|---------|
| `DASHBOARD_DB_URL` | `postgres://app_core:...@db-private-ip:5432/networker_core` | Core database |
| `DASHBOARD_LOGS_DB_URL` | Derived from `DASHBOARD_DB_URL` (replace DB name with `networker_logs`, user with `app_logs`) | Logs database |

### Pool sizing

- Core: `max_size: 16` (unchanged)
- Logs: `max_size: 8` (write-heavy but simple queries)

### Code changes

All existing DB module files continue using `core_pool`. Only two files switch to `logs_pool`:
- `benchmark_progress.rs`
- `perf_log.rs`

## 4. Database Security (Defense in Depth)

### Layer 1 — Network isolation (Azure NSG)

- db-server has **no public IP**
- NSG rules: allow TCP 5432 **only** from web-server's private IP within the VNet
- All other inbound traffic denied

### Layer 2 — PostgreSQL authentication

- `pg_hba.conf`: `hostssl` only (no unencrypted connections)
- Separate DB users with least privilege:

| User | Access | Purpose |
|------|--------|---------|
| `app_core` | CRUD on `networker_core` | Web-server application |
| `app_logs` | CRUD on `networker_logs` | Web-server application |
| `backup_user` | Read-only on `networker_core` + WAL archiving | pg_dump cron |
| `dev_admin` | Read-only on both databases | Dev access via SSH tunnel |
| `dev_write` | Read-write on both databases, requires explicit `SET ROLE` | Exceptional dev access |

- No `postgres` superuser login over network — local socket only

### Layer 3 — TLS for connections

- PostgreSQL configured with `ssl = on` (self-signed cert initially)
- Web-server connects with `sslmode=require` in connection strings

### Layer 4 — Dev access

SSH tunnel through web-server (no VPN, no public DB exposure):

```bash
ssh -L 5432:db-private-ip:5432 azureuser@20.42.8.158
psql -h localhost -U dev_admin -d networker_core
```

Future: add Azure P2S VPN when team grows.

## 5. Backup Strategy (GFS)

All backups run on db-server, push to Azure Blob Storage.

| Tier | Method | Schedule | Retention | Blob path |
|------|--------|----------|-----------|-----------|
| **Continuous** | WAL archiving | Streaming | 24 hours | `backups/wal/` |
| **Daily** | `pg_dump networker_core \| gzip` | 02:00 UTC cron | 30 days | `backups/daily/YYYY-MM-DD.sql.gz` |
| **Monthly** | 1st-of-month daily dump tagged, exempt from 30-day cleanup | Automatic | 12 months | `backups/monthly/YYYY-MM.sql.gz` |

`networker_logs` is **never backed up**.

### Azure Blob lifecycle policy

- Delete `backups/daily/*` older than 30 days
- Delete `backups/wal/*` older than 24 hours
- Delete `backups/monthly/*` older than 365 days
- Lifecycle policy enforced by Azure — no application-side cron needed for cleanup

### Backup verification

After each daily backup:
1. Write `backups/last_backup.json` to Blob with timestamp, size, and checksum
2. Scheduler on web-server reads this marker hourly to verify backup freshness

## 6. Logs Retention

Daily cron on db-server at 03:00 UTC (after backup completes):

```sql
DELETE FROM benchmark_request_progress WHERE created_at < now() - interval '7 days';
DELETE FROM perf_log WHERE logged_at < now() - interval '7 days';
VACUUM ANALYZE benchmark_request_progress;
VACUUM ANALYZE perf_log;
```

## 7. Disaster Recovery CLI (`scripts/infra.sh`)

Single entry point for all infrastructure operations.

### Commands

```bash
# Full production restore — new db-server + web-server from scratch
./infra.sh restore --env prod --confirm-production

# Restore just the database server
./infra.sh restore --env prod --db-only --confirm-production

# Point-in-time recovery
./infra.sh restore --env prod --point-in-time "2026-04-07 14:30:00" --confirm-production

# Create sandbox with anonymized prod data
./infra.sh sandbox --name dev-igor

# Tear down a sandbox
./infra.sh sandbox --name dev-igor --destroy

# List all running sandboxes
./infra.sh list
```

### Full restore flow

| Step | Action | ~Time |
|------|--------|-------|
| 1 | Provision db-server VM from pre-baked Azure image | ~3 min |
| 2 | Download latest backup from Blob | ~2 min |
| 3 | `pg_restore` networker_core | ~2 min |
| 4 | Replay WAL if point-in-time mode | ~3 min |
| 5 | Create empty `networker_logs` with schema | ~10 sec |
| 6 | Apply pg_hba.conf, TLS, DB users, WAL archiving | ~1 min |
| 7 | Provision web-server VM from pre-baked image (unless `--db-only`) | ~3 min |
| 8 | Deploy binaries from latest GitHub release tag | ~1 min |
| 9 | Configure connection strings to new db-server | ~10 sec |
| 10 | Run health checks, print status report | ~30 sec |

**Target RTO: ~15 minutes.**

### Sandbox flow

| Step | Action |
|------|--------|
| 1 | Provision single VM (DB + app together — it's dev) |
| 2 | Restore `networker_core` from latest backup |
| 3 | Run anonymization pass (see below) |
| 4 | Create empty `networker_logs` |
| 5 | Deploy binaries, start services |
| 6 | Print access URL + sandbox credentials |

### Data anonymization (sandbox only)

| Table | Field | Anonymization |
|-------|-------|---------------|
| `dash_user` | email | `user_{id}@sandbox.local` |
| `dash_user` | display_name | `User {id}` |
| `dash_user` | password_hash | Reset to shared sandbox password |
| `dash_user` | avatar_url | `NULL` |
| `cloud_account` | credentials_enc, credentials_nonce | DELETE all rows |
| `cloud_connection` | * | DELETE all rows |
| `workspace_invite` | * | DELETE all rows |
| `share_link` | * | DELETE all rows |
| `dash_user` | password_reset_token, password_reset_expires | `NULL` |

Everything else (projects, test definitions, benchmark results, configs) preserved — that's the valuable data for debugging.

### Safety rails

- `restore --env prod` requires `--confirm-production` flag
- Sandbox VMs auto-shutdown after 48 hours (configurable with `--ttl`)
- Sandbox names prefixed `sandbox-` in Azure, tagged `env=sandbox` for easy cleanup
- `infra.sh list` shows all running sandboxes with age and estimated cost

## 8. Monitoring & Health

The existing scheduler on web-server gets a new hourly health check task.

### Health checks

| Check | Source | Healthy when |
|-------|--------|-------------|
| Core DB connectivity | `SELECT 1` on core_pool | Responds < 2s |
| Logs DB connectivity | `SELECT 1` on logs_pool | Responds < 2s |
| Core DB size | `pg_database_size('networker_core')` | Below 5GB (configurable) |
| Logs DB size | `pg_database_size('networker_logs')` | Below 2GB (configurable) |
| Logs retention | `MIN(created_at)` from perf_log | Oldest row < 8 days |
| Last backup age | `last_backup.json` from Azure Blob | < 25 hours |
| WAL archive lag | `pg_stat_archiver` on core_pool | `last_archived_time` < 10 min ago |
| Backup size trend | Compare latest vs previous daily dump | Alert if > 50% growth |

### Storage

Results written to `system_health` table in `networker_core` (rolling 7 days).

### Frontend

Settings page gets a **System Health** card:
- Green/yellow/red status for each check
- Last backup timestamp + size
- DB sizes with sparkline history
- One-click "Test backup restore" button (creates temp sandbox, verifies, tears down)

## 9. Docker Compose (Local Dev)

Updated `docker-compose.dashboard.yml`:

```yaml
postgres:
  image: postgres:16-alpine
  environment:
    POSTGRES_DB: networker_core
    POSTGRES_USER: networker
    POSTGRES_PASSWORD: networker
  ports:
    - "5432:5432"
  volumes:
    - pgdata:/var/lib/postgresql/data
    - ./scripts/init-logs-db.sql:/docker-entrypoint-initdb.d/01-logs-db.sql
  healthcheck:
    test: ["CMD-SHELL", "pg_isready -U networker -d networker_core"]
    interval: 5s
    timeout: 5s
    retries: 5
```

`scripts/init-logs-db.sql`:
```sql
CREATE DATABASE networker_logs OWNER networker;
```

Local dev uses the same PostgreSQL instance with both databases — mirrors production topology without needing two containers.

## 10. Files to Create/Modify

### New files
- `scripts/infra.sh` — DR CLI (restore, sandbox, list)
- `scripts/init-logs-db.sql` — Docker entrypoint script for logs DB
- `scripts/anonymize.sql` — Sandbox data anonymization queries
- `scripts/backup-daily.sh` — pg_dump + upload to Blob (cron on db-server)
- `scripts/backup-wal.sh` — WAL archive command for PostgreSQL
- `scripts/retention-cleanup.sh` — 7-day log deletion + vacuum

### Modified files
- `crates/networker-dashboard/src/db/mod.rs` — Add `logs_pool`, `create_logs_pool()`
- `crates/networker-dashboard/src/db/migrations.rs` — V026: create logs DB, move tables
- `crates/networker-dashboard/src/db/benchmark_progress.rs` — Use `logs_pool`
- `crates/networker-dashboard/src/db/perf_log.rs` — Use `logs_pool`
- `crates/networker-dashboard/src/config.rs` — Add `DASHBOARD_LOGS_DB_URL`
- `crates/networker-dashboard/src/state.rs` (or wherever DashboardState lives) — Add `logs_pool` field
- `crates/networker-dashboard/src/scheduler.rs` — Add health check task
- `crates/networker-dashboard/src/api/` — Add `/api/system/health` endpoint
- `docker-compose.dashboard.yml` — Rename DB, add init script
- `dashboard/` — System Health card on Settings page

## 11. Future Path (Multi-Region)

This design is explicitly forward-compatible with sovereignty zones:

- Each datacenter gets its own db-server + web-server pair
- `server_registry.db_url` already stores per-server database URLs
- Replicate `networker_core` across datacenters (PostgreSQL logical replication)
- `networker_logs` stays local per datacenter — never replicated
- Azure Front Door routes to the closest web-server
- `infra.sh restore` gains `--zone` flag to target a specific datacenter
- Replace SSH tunnel dev access with Azure P2S VPN when team grows
