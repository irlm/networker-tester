# Runbook: Operating the control plane — every signal

One page pointing an operator at every health/observability signal for the C#
control plane (`alethedash-cs` systemd service on the control-plane VM).

## Health endpoints

| Endpoint | Meaning |
|---|---|
| `GET /api/health` | Liveness — DB reachable, returns `ok`. Used by the deploy health check and the frontend connection dot. |
| `GET /api/health/ready` | Readiness — `200 {"status":"ready"}` when the instance can serve traffic (wired into deploy / LB). |
| `GET /api/health/background` | Per-replica background-loop status from the `TickMonitor` — shows each loop's last tick so you can see whether the scheduler/watchdog/reaper are ticking on this replica. |

`/api/health/background` is per-replica (each replica reports only its own ticks);
loops run under per-tick Postgres advisory-lock leader election, so on any given
tick only one replica does the work.

## Logs

- Service logs: `journalctl -u alethedash-cs -f`.
- Request timing: the `perf_log` table (see
  [`perf-log-diagnosis.md`](perf-log-diagnosis.md)); every response carries the
  `X-Process-Time-Ms` header.

## Watchdog / reaper WARN messages

`WatchdogService` (`src/Networker.ControlPlane/Background/WatchdogService.cs`)
logs a WARN each time it reaps stuck work. What each means:

| Log message | Trigger | Cutoff |
|---|---|---|
| `Reaped stale running run {RunId} — agent {WorkerId} offline` | A `running` run whose agent is no longer connected | 120 s |
| `Reaped stale queued run {RunId} — no runner claimed it within {Cutoff}s` | A `queued` run no runner claimed | 300 s |
| `Reaped stale deployment {DeploymentId} — pending/running for more than {Cutoff}m (control plane likely restarted mid-deploy)` | A deployment stuck `pending`/`running` | **30 min** |
| `Reaped orphaned provisioning run {RunId} — its deployment {DeploymentId} is gone/missing` | A `provisioning` run whose deployment no longer exists | 30 min |

A burst of these after a restart is expected recovery (the 30-min deployment sweep
is what unblocks an orphaned deploy). A steady stream of stale-running/queued
reaps points at agent connectivity or dispatch problems.

## Related

- Run-lifecycle guarantees and the full watchdog table:
  [`../architecture.md`](../architecture.md) (Run Lifecycle & Reliability Guarantees).
- Production ops (leader election, soak, rollback, decommission):
  [`../phase2-cutover-runbook.md`](../phase2-cutover-runbook.md).
