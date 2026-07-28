# Runbook: Operating the control plane — every signal

This one page points an operator at every health and observability signal for
the C# control plane. The control plane is the `alethedash-cs` systemd service
on the control-plane VM.

## Health endpoints

| Endpoint | Meaning |
|---|---|
| `GET /api/health` | Liveness — DB reachable, returns `ok`. Used by the deploy health check and the frontend connection dot. |
| `GET /api/health/ready` | Readiness — `200 {"status":"ready"}` when the instance can serve traffic (wired into deploy / LB). |
| `GET /api/health/background` | Per-replica background-loop status from the `TickMonitor` — shows each loop's last tick so you can see whether the scheduler/watchdog/reaper are ticking on this replica. |

`/api/health/background` is per-replica. Each replica reports only its own ticks.
The loops run under per-tick Postgres advisory-lock leader election. Therefore,
only one replica does the work on any one tick.

## Logs

- Service logs: `journalctl -u alethedash-cs -f`.
- Request timing: the `perf_log` table (see
  [`perf-log-diagnosis.md`](perf-log-diagnosis.md)); every response carries the
  `X-Process-Time-Ms` header.

## Watchdog / reaper WARN messages

`WatchdogService` (`src/Networker.ControlPlane/Background/WatchdogService.cs`)
logs a WARN each time it reaps stuck work. Each message has this meaning:

| Log message | Trigger | Cutoff |
|---|---|---|
| `Reaped stale running run {RunId} — agent {WorkerId} offline` | A `running` run whose agent is no longer connected | 120 s |
| `Reaped stale queued run {RunId} — no runner claimed it within {Cutoff}s` | A `queued` run no runner claimed | 300 s |
| `Reaped stale deployment {DeploymentId} — pending/running for more than {Cutoff}m (control plane likely restarted mid-deploy)` | A deployment stuck `pending`/`running` | **30 min** |
| `Reaped orphaned provisioning run {RunId} — its deployment {DeploymentId} is gone/missing` | A `provisioning` run whose deployment no longer exists | 30 min |

A burst of these messages after a restart is normal recovery. The 30-min
deployment sweep unblocks an orphaned deploy. A steady stream of stale-running
or stale-queued reaps shows agent connectivity or dispatch problems.

## Automated monitoring

Two GitHub Actions watch production. They page the watchers on an outage:

| Workflow | Cadence | Checks | On failure |
|---|---|---|---|
| **Prod uptime monitor** (`.github/workflows/uptime-monitor.yml`) | ~10 min | `/api/health` is 200 + `status=ok` + `db=ok` on both domains; SPA root + API routes don't 5xx/502/503; TLS cert not expiring (<7 d fails, <14 d warns) | Opens/updates the deduped issue **"Prod uptime monitor failing"** (pinned), closes it on recovery |
| **Prod soak check** (`.github/workflows/soak-check.yml`) | Nightly | Deep on-VM checklist: background loops healthy, no stuck-queued runs, no Rust writers, advisory-lock count, orphan-resource count; also the decommission soak streak | Opens/updates the deduped issue **"Soak check failing"** |

The uptime monitor uses public endpoints only and needs no secrets. The soak
needs `AZURE_CREDENTIALS` and `DASHBOARD_ADMIN_PASSWORD`. Keep
`DASHBOARD_ADMIN_PASSWORD` in sync after an
[admin password reset](admin-password-reset.md). If you do not, the soak login
breaks. Neither workflow watches the disk space or the DB growth on the VM. For
those, use `df -h` and `pg_database_size()` through `az vm run-command`. This
check is not yet automated.

## Related

- Run-lifecycle guarantees and the full watchdog table:
  [`../architecture.md`](../architecture.md) (Run Lifecycle & Reliability Guarantees).
- Production ops (leader election, soak, rollback, decommission):
  [`../phase2-cutover-runbook.md`](../phase2-cutover-runbook.md).
