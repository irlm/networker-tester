# Monitoring Plane and Incident Management Design

**Status:** Accepted for staged implementation

**Date:** 2026-08-18

**Scope:** Independent API monitoring, reports, opt-in alerting, incident
management, and outage-safe diagnostic logs

## 1. Decision summary

LagHound will add a separately deployable **Monitoring Plane**. It is managed
from the normal LagHound dashboard when the control plane is healthy and from
an independently hosted monitoring console when it is not.

The central product distinction is:

- **Monitoring observes every configured API.** Every monitor produces checks,
  history, latency summaries, availability reports, and regression evidence.
- **Alerting is opt-in.** Operators add alert policies only to important
  monitors. Reporting-only monitors never page anyone.
- **Incidents represent sustained critical failures.** A single bad check or a
  warning regression remains evidence in the report; it does not create an
  incident.

The approved v1 choices are:

1. The data model supports multiple locations, while the MVP requires only one
   selected monitoring runner per monitor.
2. HTTP checks support status, latency, response-header, and JSON-path
   assertions.
3. Sustained critical failures create incidents automatically. Warning-level
   regressions remain report-only.

## 2. Why this must be a separate system

A background service inside `Networker.ControlPlane` would stop when the
control plane, its VM, or its database stops. That is the exact failure the
monitor must detect. A separate process that still shares the same database or
host has the same blind spot.

The Monitoring Plane therefore owns its runtime, durable state, scheduler,
probe execution, incident state machine, notification outbox, and diagnostic
log store. Normal operation may use the same product navigation and visual
components, but it must not require a live call to the main control plane.

The existing public uptime workflow remains valuable. No service can be its
own only observer, so an external watcher must continue checking the Monitoring
Plane itself.

## 3. Goals and non-goals

### Goals

- Continue API checks when the main control plane is unavailable.
- Let operators create and manage API monitors from the LagHound dashboard.
- Provide an independent console for outage investigation.
- Preserve every monitor's useful history even when alerting is disabled.
- Detect availability, correctness, and latency failures with explicit
  evidence.
- Prevent one transient failure from opening an incident.
- Deduplicate incident and notification delivery under retries and concurrent
  workers.
- Correlate checks with logs that were shipped before or during the outage.
- Keep the schema and API ready for multiple probe locations without requiring
  a multi-location MVP.
- Reuse the Rust measurement engine, design-system components, and notification
  delivery logic without sharing the control plane's failure domain.

### Non-goals for v1

- A public customer-facing status page.
- Browser transaction scripting or arbitrary JavaScript execution.
- Full distributed tracing storage.
- Arbitrary response-body archival.
- Automatic remediation or deployment rollback.
- A hosted global probe fleet operated by the LagHound project.
- Migration of existing run-based alert rules out of the control plane.

## 4. Runtime topology

```text
                           normal management path
  LagHound dashboard  ───────────────────────────────────┐
                                                        │
  Independent console ──────────────────────────────────┤
  (separate static host)                                 ▼
                                              ┌───────────────────────┐
                                              │ Monitoring API        │
                                              │ + scheduler           │
                                              │ + incident evaluator  │
                                              │ + notification outbox │
                                              └───────────┬───────────┘
                                                          │
                                  ┌───────────────────────┼──────────────┐
                                  ▼                       ▼              ▼
                         Monitoring database       Probe runner    Log ingestion
                         separate credentials      MVP: one        + bounded spool
                         separate failure domain   future: many
                                                          │
                                                          ▼
                                                    Monitored APIs

  Main services ── structured log shipper ──────────────────────────────▲
  External uptime workflow ── checks Monitoring Plane `/health` directly
```

### Deployment boundary

The Monitoring Plane ships as its own executable and container, tentatively
`Networker.Monitoring`. It may share source packages and contracts with the
control plane, but it must not import or start `Networker.ControlPlane`.

For the flagship deployment it runs on a different VM or managed compute
instance and uses PostgreSQL in a different failure domain. A development or
small self-hosted installation may colocate the services, but the UI and
documentation must label that mode as **not outage independent**.

The probe runner also runs independently from the main agent dispatch loop. It
may invoke the existing Rust probe binary as a child process or linked command,
but monitor scheduling and result persistence belong to the Monitoring Plane.

## 5. Ownership boundaries

| Capability | Owner | Must work while control plane is down? |
|---|---|---:|
| Monitor definitions and secrets | Monitoring Plane | Yes |
| Monitor schedule and leases | Monitoring Plane | Yes |
| Probe execution | Monitoring runner | Yes |
| Check results and rollups | Monitoring database | Yes |
| Monitor alert policies | Monitoring Plane | Yes |
| Incidents and incident timeline | Monitoring Plane | Yes |
| Notification delivery and retries | Monitoring Plane | Yes |
| Monitoring diagnostic logs | Monitoring Plane | Yes |
| Existing test-run alert rules | Control plane | No change in v1 |
| Existing benchmark regressions | Control plane | No change in v1 |
| Integrated navigation and shared UI primitives | Dashboard source | No; fallback console exists |

The Monitoring Plane is the system of record for its configuration. The main
dashboard calls its API; it does not persist a second authoritative copy in the
control-plane database.

## 6. Domain model

IDs are UUIDs unless stated otherwise. Every mutation carries an idempotency
key at the API boundary, and every table includes `created_at` and `updated_at`
where applicable.

### `monitor`

One API endpoint and its check contract.

| Field | Purpose |
|---|---|
| `monitor_id` | Stable identity |
| `project_id` | Existing LagHound project identity |
| `name` | Operator-facing name |
| `target_url` | HTTPS/HTTP target after validation |
| `method` | `GET`, `HEAD`, `POST`, `PUT`, or `PATCH`; default `GET` |
| `request_config_encrypted` | Sensitive headers and optional request body |
| `assertion_config` | Versioned status/header/JSON/latency assertions |
| `interval_seconds` | Check cadence; v1 minimum 60 seconds |
| `timeout_ms` | Whole-check timeout |
| `enabled` | Scheduling switch |
| `criticality` | `reporting` or `critical` |
| `retention_policy` | Raw and rollup retention override |

`criticality=reporting` means checks and reports only. It does not prevent an
operator from adding an alert policy later.

### `monitor_location`

The multi-location-ready assignment model.

| Field | Purpose |
|---|---|
| `location_id` | Stable runner/location identity |
| `name` | Human-readable location |
| `provider`, `region` | Optional placement metadata |
| `last_heartbeat_at` | Runner liveness |
| `enabled` | Scheduling eligibility |

### `monitor_location_assignment`

Links monitors to locations and records whether the location participates in
the incident quorum. V1 creates exactly one active assignment per monitor.

### `monitor_check`

An immutable execution result.

| Field | Purpose |
|---|---|
| `check_id` | Idempotent execution identity |
| `monitor_id`, `location_id` | Source |
| `scheduled_at`, `started_at`, `finished_at` | Timing and scheduler lag |
| `outcome` | `healthy`, `warning`, `critical`, or `unknown` |
| `failure_kind` | DNS, connect, TLS, timeout, status, header, JSON, latency, runner |
| `status_code` | HTTP response status when available |
| `dns_ms`, `connect_ms`, `tls_ms`, `ttfb_ms`, `total_ms` | Phase timings |
| `assertion_results` | Versioned per-assertion outcomes |
| `response_digest` | Digest only; response body is not stored by default |
| `error_summary` | Redacted, bounded diagnostic message |

`unknown` means LagHound could not establish target health, for example
because the monitoring runner was unavailable. Unknown is never interpreted as
healthy and does not directly create a target incident.

### `monitor_rollup`

Hourly and daily aggregates used by reports: check count, outcome counts,
availability, latency percentiles, assertion failure counts, and location
coverage. Raw checks default to 30-day retention; daily rollups default to 13
months. Both are configurable.

### `monitor_alert_policy`

An optional policy attached to one monitor.

| Field | Purpose |
|---|---|
| `policy_id`, `monitor_id` | Identity and scope |
| `enabled` | Notification/incident switch |
| `critical_window` | Default: 2 critical results among the last 3 checks |
| `recovery_window` | Default: 3 consecutive healthy checks |
| `latency_warning_ms` | Report-only warning threshold |
| `latency_critical_ms` | Incident-eligible latency threshold |
| `channel_ids` | One or more independent notification channels |

### `incident`

One durable operational episode, not one failed check.

| Field | Purpose |
|---|---|
| `incident_id`, `project_id`, `monitor_id` | Identity and ownership |
| `fingerprint` | Stable monitor + failure-category dedup key |
| `status` | `open`, `acknowledged`, `monitoring`, `resolved` |
| `severity` | V1 incidents are `critical` |
| `summary` | Human-readable failure statement |
| `opened_at`, `acknowledged_at`, `resolved_at` | Lifecycle timing |
| `last_seen_at` | Latest contributing failure |
| `acknowledged_by`, `resolved_by` | Optional operator identity |
| `opening_check_id`, `latest_check_id` | Evidence anchors |

A partial unique index permits only one unresolved incident for a
`(monitor_id, fingerprint)` pair.

### `incident_event`

Append-only timeline entries: opened, evidence attached, notification queued,
notification delivered/failed, acknowledged, moved to monitoring, auto-
resolved, manually resolved, and reopened.

### `notification_channel`, `notification_outbox`

The Monitoring Plane owns independent email and signed-webhook channels. The
implementation should extract reusable notifier code from the control plane,
not read control-plane alert tables at runtime.

The outbox is durable. Incident state commits and notification enqueue happen
in one database transaction. A worker leases pending rows, retries with
bounded exponential backoff, records every attempt, and moves exhausted rows
to a visible dead-letter state.

### `monitor_log_entry`

Bounded, structured logs shipped to the Monitoring Plane. Required fields are
timestamp, source, level, message template, rendered message, service,
environment, and optional `check_id`, `incident_id`, `trace_id`, and
`request_id` correlation identifiers.

## 7. Check evaluation

### Assertions

The v1 assertion contract supports:

- Allowed status codes or ranges; default `200..399`.
- Maximum warning and critical total latency.
- Header existence, equality, substring, and regular-expression checks.
- JSON-path existence, equality, numeric comparison, and regular-expression
  checks.

The checker reads at most a configured bounded response size, default 64 KiB,
to evaluate body assertions. It stores assertion outcomes and a response digest,
not the body. An explicitly enabled failure sample must be size-limited,
redacted, encrypted at rest, and excluded from reports by default.

### Outcome precedence

1. Probe infrastructure unavailable -> `unknown`.
2. DNS/connect/TLS/timeout/status/body correctness failure -> `critical`.
3. Critical latency threshold exceeded -> `critical`.
4. Warning latency threshold or non-critical regression exceeded -> `warning`.
5. Otherwise -> `healthy`.

### Incident state machine

```text
healthy/reporting
  └─ critical window met ─> open
                               ├─ operator acknowledges ─> acknowledged
                               ├─ first healthy evidence ─> monitoring
                               └─ critical again ─────────> open/acknowledged

monitoring
  ├─ recovery window met ─> resolved
  └─ critical result ─────> open
```

Defaults:

- Open after **2 critical results among the last 3 scheduled checks**.
- Resolve after **3 consecutive healthy scheduled checks**.
- A warning never opens an incident.
- An unknown result pauses the evidence window; it neither confirms failure
  nor recovery.
- Repeated critical checks update the existing incident and do not resend the
  opening notification. Policies may send bounded reminders later.
- A critical result after resolution opens a new incident linked to the prior
  incident as a recurrence.

Multi-location quorum is deferred, but the event model records location from
day one so v2 can require failures from more than one location.

## 8. Reports versus alerts

Every monitor receives reports regardless of criticality or alert policy:

- Availability by hour/day and selected range.
- p50/p95/p99 total latency and phase timing.
- Warning and critical check counts.
- Assertion failure breakdown.
- Location coverage and unknown periods.
- Change versus the previous comparable period.
- Incident markers when incidents exist.

An alert policy adds operational actions only:

- Incident creation for sustained critical evidence.
- Email/webhook delivery.
- Acknowledgement and resolution workflow.
- Optional reminders and escalation in later versions.

This separation prevents low-priority APIs from creating alert fatigue while
retaining evidence needed for capacity and regression reviews.

## 9. Logs and incident evidence

The main services cannot be the only place their diagnostic logs live. A
lightweight shipper batches structured logs to the Monitoring Plane and keeps a
bounded on-disk spool when the receiver is temporarily unavailable.

Monitor requests include `X-LagHound-Check-Id`. Services controlled by the user
may copy that identifier into application logs. The incident page then queries
logs by `check_id`; where explicit correlation is unavailable, it may show a
clearly labelled time-window match for the configured service.

An incident evidence bundle contains:

- The opening check and the preceding five checks.
- Phase timings, status, and assertion results.
- Runner/location metadata and heartbeat state.
- Baseline/report deltas.
- Monitoring-runner logs for the check.
- Correlated application logs, when shipped.
- Notification attempts and outcomes.

Logs default to seven-day retention. Resolving an incident preserves the
bounded evidence bundle even after the source log retention window expires.

Secrets, authorization headers, cookies, query-string credentials, and raw
response bodies are redacted before persistence. The UI must state when logs
are unavailable rather than implying that silence means health.

## 10. Security boundary

Persistent HTTP monitoring is an SSRF-capable feature unless constrained.
Before any request and after every redirect, the runner resolves the hostname
and rejects disallowed addresses. DNS rebinding defenses validate every
resolved address used for the connection.

Defaults:

- Reject loopback, link-local, multicast, unspecified, and cloud metadata
  destinations.
- Reject RFC1918/private destinations unless the selected self-hosted
  monitoring location explicitly allows private targets.
- Permit at most three redirects and revalidate each destination.
- Restrict methods and body sizes.
- Encrypt request secrets with a Monitoring Plane key distinct from the main
  control-plane credential key.
- Never return stored secrets through read APIs.
- Apply project-role authorization locally without calling the main control
  plane per request.
- Record every monitor, policy, incident, and channel mutation in an audit log.

Normal dashboard sessions and the independent console validate signed identity
locally. Production emergency access must use an identity provider reachable
without the main control plane. A self-hosted deployment that relies only on
main-control-plane local login is not fully outage independent and must be
labelled accordingly.

## 11. API contract

The independent service exposes `/api/v1`. The integrated deployment may map
it at `/api/monitoring/v1`; the standalone console calls the Monitoring Plane
origin directly. Wire fields are `snake_case`, errors use `{ "error": "..." }`,
and list endpoints use cursor pagination.

### Monitors and checks

| Method and route | Purpose |
|---|---|
| `POST /projects/{projectId}/monitors` | Create a monitor |
| `GET /projects/{projectId}/monitors` | List monitors with latest status |
| `GET /monitors/{monitorId}` | Monitor detail |
| `PATCH /monitors/{monitorId}` | Update configuration or pause/resume |
| `DELETE /monitors/{monitorId}` | Soft-delete and stop scheduling |
| `POST /monitors/{monitorId}/test` | Run an immediate non-incidenting validation |
| `GET /monitors/{monitorId}/checks` | Paginated check history |
| `GET /checks/{checkId}` | Full redacted check evidence |
| `GET /monitors/{monitorId}/reports` | Rollups for a requested time range |
| `GET /locations` | Available monitoring locations |

The test endpoint verifies configuration but is explicitly excluded from the
incident evidence window.

### Alert policies and channels

| Method and route | Purpose |
|---|---|
| `POST /monitors/{monitorId}/alert-policy` | Add opt-in alerting |
| `GET/PATCH/DELETE /alert-policies/{policyId}` | Read or manage policy |
| `POST /projects/{projectId}/channels` | Create email/webhook channel |
| `GET /projects/{projectId}/channels` | List channels |
| `PATCH/DELETE /channels/{channelId}` | Manage channel |
| `POST /channels/{channelId}/test` | Queue test delivery |

### Incidents and logs

| Method and route | Purpose |
|---|---|
| `GET /projects/{projectId}/incidents` | Filterable incident list |
| `GET /incidents/{incidentId}` | Incident and timeline |
| `POST /incidents/{incidentId}/acknowledge` | Acknowledge with optional note |
| `POST /incidents/{incidentId}/resolve` | Manual resolution with note |
| `GET /incidents/{incidentId}/logs` | Correlated, redacted logs |
| `POST /ingest/logs` | Authenticated batched log ingestion |
| `GET /health` | External Monitoring Plane liveness/readiness summary |

## 12. User experience

### Monitoring

The project navigation adds **Monitoring**. Its overview prioritizes current
state rather than configuration:

- Open incidents and monitors requiring attention.
- Monitor table with name, endpoint, current status, last check, availability,
  p95, location, and alerting state.
- Filters for status, alerting enabled, location, and time range.
- Primary action: **Create monitor**.

The creation flow uses progressive disclosure:

1. **Endpoint:** name, URL, and method.
2. **Request and assertions:** headers/auth, optional request body, expected
   status, latency, headers, and JSON-path rules.
3. **Execution:** one location, cadence, and timeout. The data contract already
   permits more locations.
4. **Review and test:** run a non-incidenting check, inspect evidence, then
   save.

Alert configuration is intentionally absent from this flow. The saved monitor
detail offers **Add alert policy** as a separate action.

The monitor detail page contains Overview, Checks, Reports, and Configuration.
Warnings remain visible in Checks and Reports even when no alert policy exists.

### Alerts

The existing Alerts experience evolves without mixing observation and action:

1. **Incidents** — open first, then resolved; the default operational view.
2. **Policies** — monitor alert policies and existing run rules, clearly
   labelled by source.
3. **Channels** — independent monitoring channels plus existing run channels,
   with source labels during the transition.
4. **History** — incident transitions and delivery history.

The independent console includes Monitoring and Alerts only. It reuses the
same components and design tokens but is built and deployed as a separate
static entry point. It must remain useful at mobile incident-response widths,
preserve raw evidence access, and never depend on control-plane API calls.

## 13. Scheduling and reliability

- The scheduler uses database leases so multiple replicas do not duplicate a
  scheduled check.
- Check IDs are allocated before dispatch and remain stable through retries.
- Results are inserted idempotently by `check_id`.
- A missed schedule becomes explicit scheduler-lag telemetry, not a healthy
  check.
- Monitoring location heartbeats determine whether target status is
  `unknown`.
- Incident evaluation and outbox enqueue occur transactionally after check
  persistence.
- Probe retries are not hidden. The stored result reports every attempt so a
  successful retry does not erase evidence of instability.
- The Monitoring Plane exposes its own scheduler, runner-heartbeat, database,
  outbox-depth, and last-successful-check health signals.
- An external workflow checks `/health` and the standalone console from a
  different failure domain.

## 14. Delivery plan

Each phase is a separate PR with its own tests and rollback boundary.

### PR 1 — architecture contract

- This design document and documentation links.
- No runtime or schema changes.

### PR 2 — independent service foundation

- `Networker.Monitoring` executable and health endpoint.
- Separate PostgreSQL schema/migrator.
- Monitor/location/check entities and management APIs.
- Scheduler lease and one-location execution using the existing probe engine.
- Status and latency assertions.

### PR 3 — assertion and execution hardening

- Header and JSON-path assertions.
- SSRF, redirect, size, timeout, and secret-storage protections.
- Immediate test-check endpoint.
- Rollups and retention.
- Multi-location-ready assignment APIs while enforcing the v1 single-location
  product limit.

### PR 4 — monitoring interfaces

- Integrated Monitoring overview/create/detail flows.
- Standalone outage console build and deployment path.
- Shared query/API layer and component reuse.
- Responsive, accessibility, route, and production-bundle tests.

### PR 5 — alerts and incidents

- Monitor alert policies and independent notification channels.
- Incident state machine, timeline, acknowledgement, and resolution.
- Durable notification outbox with retries and delivery history.
- Alerts navigation updated to Incidents, Policies, Channels, and History.

### PR 6 — external logs and evidence

- Structured log-ingestion API and bounded on-disk shipper spool.
- Check/request correlation.
- Incident evidence bundles and retention behavior.
- Log redaction, authorization, load, and outage tests.

## 15. Acceptance criteria

The feature is not complete until the following scenario passes in an
automated lab or canary environment:

1. Create a reporting-only monitor and confirm it produces checks and reports
   without creating incidents or notifications.
2. Add an alert policy and force one critical check; confirm no incident.
3. Meet the 2-of-3 critical window; confirm exactly one incident and one
   opening notification despite concurrent evaluator retries.
4. Keep failing; confirm the incident updates without duplicate opening
   notifications.
5. Restore the API; confirm `monitoring` then automatic resolution after three
   healthy checks.
6. Stop the main control plane and its database.
7. Confirm scheduled checks continue, a new incident can open, notifications
   deliver, and the standalone console displays checks, incident evidence, and
   previously shipped logs.
8. Stop the monitoring runner; confirm the target becomes `unknown`, not
   healthy or down, and the external Monitoring Plane health check detects the
   loss of coverage.
9. Attempt loopback, metadata-service, DNS-rebinding, redirect, oversized-body,
   and secret-exfiltration cases; confirm they are rejected and audited.

## 16. Rejected alternatives

### Put monitoring in the control-plane scheduler

Rejected because a control-plane or core-database outage would stop checks,
incident creation, alerts, and access to evidence together.

### Run a separate process but share the control-plane database

Rejected because it preserves the most important shared failure mode.

### Reuse existing scheduled test runs as the only monitor execution path

Rejected because dispatch still depends on the control plane. The Monitoring
Plane may reuse the probe engine and result contracts, but owns dispatch.

### Create an incident for every failed check

Rejected because transient network failures would create alert fatigue and
destroy operator trust. Raw failures remain visible in reports.

### Store full response bodies and all logs with every check

Rejected because it creates secret, privacy, storage, and retention risks.
Store bounded assertion evidence and correlate to separately retained logs.

## 17. Documentation impact

Implementation PRs must keep these sources aligned:

- This document: architecture and product semantics.
- `docs/alerting.md`: distinction between run alerts and monitor incidents.
- `docs/runbooks/observability.md`: operating and externally monitoring the
  Monitoring Plane.
- `docs/schema-ownership.md`: independent database and migrations.
- `dashboard/ARCHITECTURE.md`: Monitoring API/query ownership and standalone
  entry point.
- `DESIGN.md`: only new reusable visual/interaction rules, not backend design.
