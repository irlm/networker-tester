# Alerting

Threshold alerting closes the monitoring loop: schedules run probes forever,
and alert rules are what turn a 3 AM latency regression into a notification
instead of a table row nobody is watching.

Wave 1 (this document) is the backend: rules, channels, evaluation, delivery,
and the REST surface. The dashboard UI ships in a follow-up wave on top of
these endpoints.

## Concepts

| Object | What it is |
|---|---|
| **Channel** (`alert_channel`) | Where notifications go. Kinds: `webhook` (HTTP POST, optionally HMAC-signed) and `email` (via the ACS sender; log-only when ACS is unconfigured). Project-scoped. |
| **Rule** (`alert_rule`) | A threshold watch: one metric of one `test_config` (or every config in the project when `test_config_id` is null), a comparator + threshold, and how many consecutive runs must breach (`window_runs`) before it fires. Points at one channel. |
| **Event** (`alert_event`) | One state transition — `firing` or `resolved` — recorded with the triggering run, the observed value, and the delivery outcome. |

### Metrics

| Metric | Source | Meaning |
|---|---|---|
| `p95_ms` | tester probe results (`RequestAttempt`/`HttpResult`) | 95th percentile latency of successful attempts in the run (HTTP total duration when available, else attempt wall time) |
| `mean_ms` | tester probe results | mean latency, same per-attempt definition |
| `error_rate` | run counters | `failure_count / (success_count + failure_count)`, 0..1 |
| `success_rate` | run counters | `success_count / (success_count + failure_count)`, 0..1 |

Comparators are strict: `gt` fires when `value > threshold`, `lt` when
`value < threshold`. A value exactly at the threshold never breaches.

### Evaluation semantics

Evaluation runs when an agent reports a run terminal (`completed` or
`failed`), inside the same `run_finished` processing path — best-effort by
contract: an evaluation or delivery failure is logged and never affects run
processing. Runs that terminate through other paths (agent disconnect
cleanup, watchdog) are not evaluated.

- **Matching** — enabled rules of the run's project whose `test_config_id` is
  null or equals the run's config.
- **Window** — the last `window_runs` terminal runs of that config (newest
  first, anchored on the run that just finished) must ALL breach. Fewer
  terminal runs than the window → no fire.
- **Missing data** — a run where the metric is not measurable (no attempts,
  no persisted probe results) breaks a breach streak and, when it is the
  triggering run itself, is skipped entirely: missing data neither fires nor
  resolves a rule.
- **Dedup / state** — state is tracked per (rule, config) via the latest
  recorded event. Quiet → breach records one `firing` event; further breaching
  runs stay silent; the first non-breaching run records one `resolved` event.
  Project-wide rules therefore track each config independently.
- **Delivery status** — recorded on the event: `delivered`, `failed: ...`
  (http status / timeout / send error), or `skipped: ...` (channel disabled
  or missing).

## API

All routes follow the v2 conventions: project-scoped collections under
`/api/v2/projects/{projectId}/...` (reads = any project member, writes =
project operator), flat per-row routes with row-level authorization (no
access reads as 404). Validation failures return the uniform
`{ "error": "..." }` envelope.

| Route | Auth | Purpose |
|---|---|---|
| `POST /api/v2/projects/{projectId}/alert-channels` | operator | create channel |
| `GET /api/v2/projects/{projectId}/alert-channels` | member | list channels |
| `PATCH /api/v2/alert-channels/{id}` | operator (row) | update name / config / enabled |
| `DELETE /api/v2/alert-channels/{id}` | operator (row) | delete; **409** while rules reference it |
| `POST /api/v2/alert-channels/{id}/test` | operator (row) | synchronous test delivery; returns `{ "delivery_status": "..." }` |
| `POST /api/v2/projects/{projectId}/alert-rules` | operator | create rule |
| `GET /api/v2/projects/{projectId}/alert-rules` | member | list rules |
| `PATCH /api/v2/alert-rules/{id}` | operator (row) | update supplied fields |
| `DELETE /api/v2/alert-rules/{id}` | operator (row) | delete (events cascade) |
| `GET /api/v2/projects/{projectId}/alert-events` | member | history, newest first; `?limit=` (≤200), `?offset=`, `?rule_id=` |

### Channel config

```jsonc
// webhook — secret optional; when set, deliveries carry the signature header.
{ "kind": "webhook", "name": "ops hook",
  "config": { "url": "https://hooks.example.com/networker", "secret": "..." } }

// email — one send per address, via the existing ACS sender
// (DASHBOARD_ACS_CONNECTION_STRING + DASHBOARD_ACS_SENDER; log-only otherwise).
{ "kind": "email", "name": "on-call",
  "config": { "to": ["sre@example.com"] } }
```

Webhook secrets are write-only: reads return `"secret": "********"`, and a
PATCH that sends the mask back keeps the stored secret.

### Rule body

```jsonc
{
  "metric": "p95_ms",            // p95_ms | mean_ms | error_rate | success_rate
  "comparator": "gt",            // gt | lt (strict)
  "threshold": 500.0,
  "window_runs": 3,              // 1..50, default 1
  "channel_id": "<channel uuid>",
  "test_config_id": "<config uuid>", // omit for every config in the project
  "enabled": true
}
```

## Webhook payload contract

Deliveries `POST` a JSON body with `Content-Type: application/json`, a 10s
timeout, and one retry. Field set (snake_case; `state` is `firing`,
`resolved`, or `test` for the channel test-fire; `test_config_id` is null for
project-wide rules; `value` is the metric observed on the triggering run):

```json
{
  "event_id":  "5cbb…",
  "rule_id":   "9f10…",
  "project_id": "usabc123def456",
  "test_config_id": "22aa…",
  "run_id":    "77dd…",
  "metric":    "p95_ms",
  "comparator": "gt",
  "threshold": 500.0,
  "value":     812.5,
  "state":     "firing",
  "message":   "p95_ms 812.5 > 500 for 3 consecutive run(s)",
  "fired_at":  "2026-07-18T03:00:00Z"
}
```

### Signature verification

When the channel config has a `secret`, every request carries:

```
X-Networker-Signature: sha256=<lowercase-hex HMAC-SHA256(secret, raw-body)>
```

The MAC is computed over the exact raw request body bytes (UTF-8). Verify
before parsing, with a constant-time compare:

```python
import hashlib, hmac

def verify(raw_body: bytes, header: str, secret: str) -> bool:
    expected = "sha256=" + hmac.new(
        secret.encode(), raw_body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

Equivalent shell check (e.g. in a debugging session):

```bash
printf '%s' "$RAW_BODY" | openssl dgst -sha256 -hmac "$SECRET" -hex
# compare against the header value after "sha256="
```

## Storage

V041 (`src/Networker.Data/Migrations/V041_alerting.sql`) owns the three
tables; see `docs/schema-ownership.md` for how migrations ship. Events
cascade with their rule and with their run; channels cannot be deleted while
rules reference them (the API answers 409).
