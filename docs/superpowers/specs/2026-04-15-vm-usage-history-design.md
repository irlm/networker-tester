# VM Usage History & Cost Tracking — Design

**Status:** approved (2026-04-15)
**Targets:** v0.27.17 → v0.27.20 (phased)
**Author:** session context 2026-04-15 (Claude + Igor)

## Problem

Today the dashboard shows the *current* state of every cloud VM (testers,
endpoints, benchmark testbeds) but drops all history the moment one is
deleted. Operators can't answer:

- *"How many hours has `bm-azure-win11` been up this month?"*
- *"What did we spend on Azure last week?"*
- *"Which deleted testers cost us the most this quarter?"*
- *"Two users linked the same AWS account — how do I see the consolidated bill?"*

We also can't reconcile the estimate we'd like to show with the real
invoice the account owner receives, so costs stay fuzzy and surprising.

## Scope

An append-only lifecycle log for every VM we create, plus a cost layer
that starts with static-rate estimates and grows into real per-account
billing reconciliation across AWS, Azure, and GCP.

### In scope

- Lifecycle events for testers, endpoints, and benchmark VMs — created,
  started, stopped, deleted, auto-shutdown, error.
- Uptime windows derived from paired events; survives VM deletion.
- Estimated cost per event (static rate table, versioned).
- Real cost reconciliation via provider billing APIs, deduped by
  provider account (not by `cloud_connection` row).
- Role-scoped visibility: viewers see aggregate, operators see their
  own resources, admins see cross-connection consolidation.
- Provider-account identity fingerprinting so duplicate connections to
  the same AWS/Azure/GCP account are recognised and their bills merged.

### Out of scope (for now)

- Chargeback/show-back to individual users.
- Cost prediction / budget alerts.
- Multi-currency display (USD only).
- Non-VM cloud resources (storage, networking, licences).

## Data model

### `vm_lifecycle` (new, append-only)

```sql
CREATE TABLE vm_lifecycle (
    event_id              UUID        PRIMARY KEY,
    project_id            TEXT        NOT NULL REFERENCES project(project_id),

    -- What the event is about
    resource_type         TEXT        NOT NULL,          -- 'tester' | 'endpoint' | 'benchmark'
    resource_id           UUID        NOT NULL,          -- tester_id / deployment_id / benchmark_config_id
    resource_name         TEXT,                          -- snapshot; not FK

    -- Where the VM lives, all snapshotted as strings so history
    -- survives renames, soft-deletes, or even hard-deletes of the
    -- source cloud_connection row.
    cloud                 TEXT        NOT NULL,          -- 'aws' | 'azure' | 'gcp'
    region                TEXT,
    vm_size               TEXT,
    vm_name               TEXT,
    vm_resource_id        TEXT,                          -- provider ARN / Azure id / GCP selfLink

    -- Link back to the connection row (may be NULL if hard-deleted;
    -- we recommend soft-delete — see `cloud_connection.deleted_at` below).
    cloud_connection_id         UUID REFERENCES cloud_connection(cloud_connection_id),
    cloud_account_name_at_event TEXT,                    -- snapshot fallback for display

    -- The real billing-account identifier. Two connections to the same
    -- account share this value, which is how we dedupe on the cost side.
    provider_account_id   TEXT,                          -- AWS account id | Azure sub id | GCP project id

    -- What happened
    event_type            TEXT        NOT NULL,
        -- 'created' | 'started' | 'stopped' | 'deleted' | 'auto_shutdown' | 'error'
    event_time            TIMESTAMPTZ NOT NULL,
    triggered_by          UUID,                          -- user_id, NULL for automatic
    metadata              JSONB,                         -- error text, shutdown reason, etc.

    created_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX vm_lifecycle_project_time ON vm_lifecycle(project_id, event_time DESC);
CREATE INDEX vm_lifecycle_resource     ON vm_lifecycle(resource_type, resource_id);
CREATE INDEX vm_lifecycle_account      ON vm_lifecycle(provider_account_id, event_time DESC);
```

**Retention policy: forever.** History never prunes automatically. Even
if a provider connection is removed, the rows stay — `cloud`, `vm_size`,
`region`, and `cloud_account_name_at_event` are snapshots, not FKs.

### `cloud_connection` (existing, add columns)

```sql
ALTER TABLE cloud_connection ADD COLUMN deleted_at          TIMESTAMPTZ;
ALTER TABLE cloud_connection ADD COLUMN provider_account_id TEXT;

CREATE UNIQUE INDEX cloud_connection_account_per_project
    ON cloud_connection(project_id, provider_account_id)
    WHERE deleted_at IS NULL;
```

- **Soft-delete:** delete handler sets `deleted_at = now()` instead of
  physically removing the row. Every existing "list connections" query
  adds `WHERE deleted_at IS NULL`.
- **Fingerprint at create:** the create handler calls the provider's
  whoami API (`sts get-caller-identity` / `az account show` /
  `gcloud config get-value project`) and stores the result.
- **Duplicate warning, not block:** if another active connection in the
  same project already has this `provider_account_id`, show a warning
  in the UI (*"This AWS account is already linked via 'Prod AWS (user A)'.
  Costs will be consolidated for admins."*). We don't block the add —
  multi-user setups genuinely need parallel connections.

### `cost_rate` (new, versioned)

```sql
CREATE TABLE cost_rate (
    cost_rate_id         UUID        PRIMARY KEY,
    cloud                TEXT        NOT NULL,    -- 'aws' | 'azure' | 'gcp'
    vm_size              TEXT        NOT NULL,    -- 't3.small' | 'Standard_D2s_v3' | 'e2-small'
    region               TEXT,                    -- NULL = any region (flat rate)
    rate_per_hour_usd    NUMERIC(12,6) NOT NULL,
    effective_from       TIMESTAMPTZ NOT NULL,
    effective_to         TIMESTAMPTZ,             -- NULL = still in effect
    source               TEXT        NOT NULL,    -- 'static-v1' | 'azure-price-api-2026-04-15' | etc.

    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX cost_rate_lookup
    ON cost_rate(cloud, vm_size, region, effective_from);
```

Rate lookups pick the row whose `[effective_from, effective_to)` covers
the event. `effective_to` is set when a newer row supersedes this one,
so historical events always price against the rate that was in effect
at the time.

When a new provider is added later, seeding is simply `INSERT` rows with
`effective_from` set to the earliest VM date for that cloud — historical
events get retroactively priced.

### `vm_cost_daily` (new, reconciliation layer)

```sql
CREATE TABLE vm_cost_daily (
    day                  DATE        NOT NULL,
    provider_account_id  TEXT        NOT NULL,
    resource_type        TEXT        NOT NULL,
    resource_id          UUID        NOT NULL,
    vm_resource_id       TEXT,

    estimated_usd        NUMERIC(12,6) NOT NULL,     -- sum of (hours up that day × rate)
    actual_usd           NUMERIC(12,6),              -- populated by reconciliation job; NULL until then
    actual_source        TEXT,                       -- 'aws-cost-explorer' | 'azure-cost-mgmt' | 'gcp-billing'
    reconciled_at        TIMESTAMPTZ,

    PRIMARY KEY (day, provider_account_id, resource_id)
);
```

Rows accumulate nightly. Monthly invoice = `SUM(COALESCE(actual_usd,
estimated_usd))` grouped by whatever dimension the user asked for.

## Lifecycle event hooks

~10 call sites emit events:

| Hook | Trigger | Event |
|---|---|---|
| `AzureProvider::create_vm`, `AwsProvider::create_vm`, `GcpProvider::create_vm` | tester/endpoint/benchmark create succeeds | `created` + `started` |
| `AzureProvider::start_vm` (+ AWS/GCP) | resume after stop | `started` |
| `AzureProvider::stop_vm` (+ AWS/GCP) | manual stop | `stopped` |
| `AzureProvider::delete_vm` (+ AWS/GCP) | manual delete | `stopped` (if running) + `deleted` |
| `auto_shutdown_loop` in `services/tester_auto_shutdown.rs` | schedule fires | `auto_shutdown` |
| `tester_state::set_error` | provisioning fails, agent timeout, etc. | `error` |
| `cloud_orphan_reaper::sweep` | reaper finds & deletes orphan | `stopped` + `deleted` with `metadata.reason='orphan_reap'` |

A single helper `services/vm_lifecycle::record_event(&mut tx, ...)`
writes the row. Publishers pass the open DB transaction so the event
commits atomically with the state change that caused it.

## Cost allocation

### Phase 1: static estimate

Seed `cost_rate` with a hand-maintained table of the ~20 VM sizes we
use (AWS `t3.*`, Azure `Standard_D*`, GCP `e2-*`, plus Windows variants).
Rates pulled from each cloud's public pricing page as of the seeding
date, `source = 'static-v1'`.

At event time, estimated cost for a completed uptime window is:

```
hours_up × rate_per_hour_usd
```

Nightly cron (`services::cost_estimator::run_daily`) computes
`vm_cost_daily.estimated_usd` for every active VM and for any VM that
transitioned the previous day.

### Phase 2: VM tagging

At create time, tag every VM with:
- `networker:project_id`
- `networker:resource_id`
- `networker:resource_type`

Migration backfills tags on existing VMs via provider tag APIs. After
this phase, cost-explorer queries filtered by tag return our VMs
precisely.

### Phase 3: real reconciliation

Nightly job, per active `cloud_connection`:

| Cloud | API | Query |
|---|---|---|
| AWS | Cost Explorer `GetCostAndUsage` | Granularity=DAILY, filter by tag `networker:resource_id`, `resource_id` dimension |
| Azure | Cost Management `Query.Usage` | scope=`/subscriptions/{sub}`, timeframe=`TheLastMonth`, groupBy tag `networker:resource_id` |
| GCP | BigQuery export of Cloud Billing | standard SQL on tagged spend view |

**Dedup by `provider_account_id`:** if two project connections map to
the same AWS account, the API is called once per day, results are
stored under the shared `provider_account_id`, and display logic shows
the allocation view each user has permission to see.

Query result lands in `vm_cost_daily.actual_usd`. UI shows both
estimate and actual side-by-side; rows with >10% discrepancy get a
badge so operators can investigate.

### Phase 4: exports

Monthly PDF + CSV export, scoped to role:
- Operator: own resources, own estimated/actual costs
- Admin: full project, per-`provider_account_id` consolidated totals,
  untagged "overhead" bucket

## Role-scoped visibility

| Role | Sees |
|---|---|
| **Viewer** | Aggregate project totals only — monthly spend, top 10 resources |
| **Operator** | Their own VMs (where `triggered_by = self` on `created` event, or `cloud_connection_id` is a connection they own), their estimated + actual costs, their uptime timelines |
| **Admin** | All project VMs + consolidated view grouped by `provider_account_id` — *"AWS Account 1234-5678: $420/mo total across 2 connections, your project's slice = $180, untagged overhead = $12"* |

## API surface

```
GET  /api/projects/{pid}/vm-history
       ?resource_type=tester&resource_id=<uuid>&from=<ts>&to=<ts>&limit=100
GET  /api/projects/{pid}/vm-history/summary
       ?group_by=resource_type|cloud|provider_account_id

GET  /api/projects/{pid}/provider-accounts                 # admin only
       -> [{provider_account_id, cloud, connections: [...], monthly_estimated, monthly_actual}]

GET  /api/projects/{pid}/cost/daily?from=<date>&to=<date>
GET  /api/projects/{pid}/cost/monthly?year=2026&month=04

POST /api/admin/cost/reconcile?cloud_connection_id=<uuid>   # manual trigger
```

All handlers check `ProjectRole` from `ProjectContext` and filter
results to the caller's visibility scope.

## UI surface

New `VM History` nav entry under **Infra**:

- Table columns: name · type · cloud/region · current status · total
  uptime hrs · last active · estimated $ · actual $ (if reconciled)
- Filters: resource_type, cloud, date range, active-vs-deleted toggle,
  connection
- Row click: detail drawer with event timeline (created → started →
  stopped → auto_shutdown → started → …) and per-day cost chart

New `Provider Accounts` nav entry (admin only):

- One row per distinct `provider_account_id`
- Shows linked connections, monthly totals, allocation breakdown
  (tagged vs overhead), reconciliation freshness

Create-connection form gains the duplicate-detection warning described
under the data-model section.

## Rollout plan

| Version | Scope |
|---|---|
| **v0.27.17** | Phase 1: `vm_lifecycle` migration, lifecycle hooks at the ~10 sites, static `cost_rate` seed, history page with filters, estimated cost column |
| **v0.27.18** | Phase 2: `cloud_connection.deleted_at` + `provider_account_id`, soft-delete handler, VM tagging at create, backfill existing VMs |
| **v0.27.19** | Phase 3: daily reconciliation job for AWS + Azure + GCP, consolidated admin view, dedup by `provider_account_id`, estimate-vs-actual badges |
| **v0.27.20** | Phase 4: PDF/CSV exports, budget alerts |

Each phase is independently shippable — v0.27.17 is useful on its own
even before tagging + reconciliation land.

## Design decisions (for the commit log)

- **Snapshot string columns, not FK-only.** History must outlive the
  `cloud_connection` row. Renames are handled via the FK+snapshot hybrid
  — prefer the FK-joined name when present, fall back to the snapshot
  when NULL.
- **Soft-delete `cloud_connection`.** Without it, dropping a connection
  would orphan every reconciled cost row pointing to it.
- **Dedup by `provider_account_id`, not by `cloud_connection_id`.** Two
  users can legitimately add separate connections to the same cloud
  account; the real bill is one invoice.
- **Rates are versioned, not overwritten.** Past events must always
  price against the rate that was in effect when they happened.
- **Nightly job, not streaming.** Provider billing APIs have minutes-to-
  hours of lag anyway; daily granularity is the useful unit. Reduces
  API quota pressure vs. per-event queries.
