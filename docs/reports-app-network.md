# Application Network Performance report

**"Is my slowness the application or the network?"** — for a LagHound SDK
endpoint (a `sdkprobe` test config) this report splits each probe's end-to-end
latency into the time the customer's application spent processing (server) and
the time spent on the wire (network), then delivers a plain-language verdict.

- API: `GET /api/projects/{projectId}/reports/app-network` (member-read — any
  project role, enforced by the `ProjectMember` policy). Optional `?config_id=`
  narrows to one SDK endpoint.
- Code: `src/Networker.ControlPlane/Endpoints/AppNetworkEndpoints.cs`
  (SQL + wire shape), `src/Networker.ControlPlane/Reports/AppNetworkLogic.cs`
  (pure verdict/split math, unit-tested without a DB).
- Feature: Wave 2 of the LagHound SDK. Wave 1 (the tester `sdkprobe` mode)
  already persists the SDK's server time; this report only **reads and
  computes** — no new persistence, no tester change.

## Where the numbers come from

The report reads the tester-owned V001 probe schema directly (raw Npgsql, like
the perf-per-cost report and the alerting metric provider). Per **successful
sdkprobe attempt** of a **completed run** it takes two measured quantities:

| Quantity | Source |
|---|---|
| `wall_ms` | `RequestAttempt.FinishedAt - StartedAt` (x 1000) — the attempt's end-to-end latency, the **same wall definition** perf-per-cost uses |
| `server_ms` | `ServerTimingResult.TotalServerMs`, joined on `AttemptId` — the LagHound SDK's `Server-Timing: total;dur`, i.e. the application's own processing time |

Attempts are filtered to `LOWER(RequestAttempt.Protocol) = 'sdkprobe'`,
`RequestAttempt.Success`, `test_run.status = 'completed'`, and a non-null
`ServerTimingResult.TotalServerMs`. A missing tester schema (`42P01`) yields an
empty, valid report — never an error.

## Formulas (also embedded in every response under `formulas`)

```
server_ms     = ServerTimingResult.TotalServerMs                 (per attempt, joined on AttemptId)
network_ms    = max(0, wall_ms - server_ms)                      (floors at 0)
split_anomaly = server_ms > wall_ms                              (counted; see below)
```

`server_ms` and `network_ms` are aggregated with Postgres `PERCENTILE_CONT`
into **median** and **p95** per group (one group per `test_config`) and overall.
`server_ratio = min(1, median_server_ms / median_wall_ms)`.

### Split anomaly

If the SDK reports a server time **greater** than the wall time the client
observed (clock skew, or the SDK timing a longer span than the round trip), the
attempt is a **split anomaly**. Its `network_ms` floors at 0 so it can never go
negative, and it is counted in `split_anomaly_count` (per group and overall).

## Verdict

Computed by `AppNetworkLogic.Verdict` from the group's (or overall) medians:

| Verdict | Condition |
|---|---|
| `server_bound` | `median_server_ms >= 0.60 * median_wall_ms` (checked first) |
| `network_bound` | `median_network_ms >= 0.60 * median_wall_ms` |
| `balanced` | neither side dominates |
| `no_data` | no sdkprobe samples for the selection |

Each verdict carries a human `main_issue` string, e.g. for `server_bound`:

> Server processing dominates: ~{server}ms of ~{total}ms — investigate your
> application, not the network.

## Response shape

```jsonc
{
  "generated_at": "2026-07-20T10:00:00Z",
  "formulas": {
    "server_ms": "server_ms = ServerTimingResult.TotalServerMs ...",
    "network_ms": "network_ms = max(0, wall_ms - server_ms) ...",
    "split": "a side is dominant (verdict) when its median >= 0.6 of median wall; else balanced",
    "split_anomaly": "split_anomaly = server_ms > wall_ms ..."
  },
  "mode": "sdkprobe",
  "attempt_count": 40,
  "split_anomaly_count": 0,
  "overall_verdict": "server_bound",
  "overall_main_issue": "Server processing dominates: ~180ms of ~220ms — investigate your application, not the network.",
  "overall_median_server_ms": 180.0,
  "overall_median_network_ms": 40.0,
  "overall_median_wall_ms": 220.0,
  "overall_server_ratio": 0.8182,
  "groups": [
    {
      "config_id": "...",
      "config_name": "checkout-api",
      "run_count": 4,
      "attempt_count": 40,
      "split_anomaly_count": 0,
      "median_server_ms": 180.0,
      "p95_server_ms": 240.0,
      "median_network_ms": 40.0,
      "p95_network_ms": 60.0,
      "median_wall_ms": 220.0,
      "server_ratio": 0.8182,
      "verdict": "server_bound",
      "main_issue": "Server processing dominates: ~180ms of ~220ms — investigate your application, not the network."
    }
  ]
}
```
