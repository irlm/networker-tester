# Provider performance-per-cost report

**"Which cloud provider gives the best performance per dollar?"** — the report
answers this from data the platform already collects: probe results from
completed runs, tester metadata (`project_tester.cloud` / `vm_size` /
`region`), and a static curated price table.

- API: `GET /api/projects/{projectId}/reports/perf-per-cost` (member-read —
  any project role, enforced by the `ProjectMember` policy)
- UI: **Value** page, `/projects/{id}/reports/value` (sidebar tail, next to Runs)
- Code: `src/Networker.ControlPlane/Endpoints/PerfPerCostEndpoints.cs`,
  `src/Networker.ControlPlane/Reports/{CloudCostTable,PerfPerCostLogic}.cs`,
  `dashboard/src/pages/ValueReportPage.tsx`

## What it computes

Aggregation is per **tester group** `(provider, vm_size, region)` × **mode
family** (`shared/modes.json`: `net`, `http`, `page`, `thru`), over the
**successful attempts of completed runs** (`test_run.status = 'completed'`,
`RequestAttempt.Success`). Runs without a bound tester are excluded (there is
nothing to price); failed attempts and non-completed runs never contribute.

Per-attempt primary metrics (tester-owned V001 schema):

| Family | Metric | Source |
|---|---|---|
| `net`, `http`, `page` | latency (ms) | `HttpResult.TotalDurationMs` when the attempt has an HTTP result, else the attempt's `StartedAt→FinishedAt` wall time — the **same definition the alerting `RunMetricProvider` uses**, so this report agrees with alert thresholds |
| `thru` | throughput (Mbps) | `HttpResult.ThroughputMbps`; if a `thru` group has no throughput samples at all it falls back to latency and says so via `metric_label` |

The report emits **median** and **p95** (Postgres `PERCENTILE_CONT`) plus run
and sample counts, so you can judge how much data is behind each number.

Known approximation: for multi-probe modes (e.g. `udp`) the attempt wall time
covers the whole probe batch, not one round-trip. The comparison is still
apples-to-apples across providers because every provider is measured the same
way.

## Value formulas

Embedded verbatim in every response (`formulas`), unit-tested in
`PerfPerCostLogicTests`:

- **`latency_cost_index = p95_ms × hourly_usd`** — dollar-weighted tail
  latency, **lower is better**. Dimension: ms·$/hr. A VM that costs twice as
  much must halve its p95 to break even. Used for `net` / `http` / `page`.
- **`mbps_per_dollar_hour = median_throughput_mbps ÷ hourly_usd`** — sustained
  megabits per dollar-hour, **higher is better**. Used for `thru`.

A value score is **never fabricated**: if the SKU has no price row or the
family has no samples, `value_score` is `null` and the UI shows `—`.

## The cost table (`shared/cloud-costs.json`)

Static, hand-curated, **no pricing API is called at runtime** (reproducible,
offline-safe, auditable). Embedded into `Networker.ControlPlane` at build time
(csproj `EmbeddedResource`); parsed + validated by `CloudCostTable` — a
malformed table fails CI (`CloudCostTableTests`) and fails loudly at first
use, never silently emits wrong economics.

Schema per row: `provider` (`azure|aws|gcp`), `sku`, `region`, `hourly_usd`
(on-demand Linux list price, USD), `source_url` (https, the page/API the
number was read from), `as_of` (date the price was verified). Top level:
`disclaimer` + table-wide `as_of`, both surfaced in the API response and the
UI footer.

Coverage: the 18 VM sizes the provisioning wizard offers
(`testbed-constants.ts` `INSTANCE_TYPES`), priced in each provider's primary
region — Azure `eastus`, AWS `us-east-1`, GCP `us-east1`.

Lookup semantics (`CloudCostTable.Find`):

1. exact `(provider, sku, region)` match wins;
2. else any row for `(provider, sku)` is used and the response flags it —
   `cost_note: "priced from eastus (no westeurope row in the cost table)"`;
3. else the group ships with `hourly_usd: null`, a `cost_note`, an entry in
   `missing_cost_skus`, and a server-side warning log. **Rows are never
   silently dropped.**

### Maintaining the table

1. Re-verify prices (quarterly, or when adding a wizard VM size):
   - Azure: Retail Prices API —
     `https://prices.azure.com/api/retail/prices?$filter=armRegionName eq 'eastus' and armSkuName eq '<SKU>' and priceType eq 'Consumption'`
     (take the Linux meter — no "Windows" in `productName`, skip Spot/Low
     Priority).
   - AWS: the on-demand feed behind the official pricing page —
     `https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/ec2/USD/current/ec2-ondemand-without-sec-sel/US East (N. Virginia)/Linux/index.json`.
   - GCP: Cloud Billing Catalog API, or `gcloud-compute.com/<region>/<sku>.html`
     (built from it) when the console renders prices client-side.
2. Edit `shared/cloud-costs.json`: update `hourly_usd`, per-row `as_of`,
   `source_url` if it moved, and the top-level `as_of`.
3. `dotnet test` — `CloudCostTableTests` re-validates the file (row count,
   providers, price sanity bounds, https sources, parseable dates, no
   duplicates, wizard-size coverage).
4. Adding a wizard size? Add the price row **and** extend
   `Embedded_table_prices_every_wizard_vm_size_in_its_primary_region`.

## Response shape (abridged)

```json
{
  "generated_at": "…",
  "cost_table": { "as_of": "2026-07-20", "disclaimer": "…", "source": "shared/cloud-costs.json (…)" },
  "formulas": { "latency_cost_index": "…", "mbps_per_dollar_hour": "…" },
  "completed_runs": 42,
  "providers_with_data": 2,
  "groups": [
    {
      "provider": "azure", "vm_size": "Standard_B2s", "region": "eastus",
      "hourly_usd": 0.0416, "cost_region": "eastus",
      "cost_source_url": "https://…", "cost_as_of": "2026-07-20", "cost_note": null,
      "families": [
        { "family": "http", "run_count": 4, "sample_count": 200,
          "metric_label": "latency_ms", "median": 42.1, "p95_ms": 120.0,
          "value_metric": "latency_cost_index", "value_score": 4.992 }
      ]
    }
  ],
  "missing_cost_skus": []
}
```

The UI shows an empty state until at least two providers have completed-run
data — a one-provider "comparison" would be noise.

## Tests

- `tests/Networker.ControlPlane.Tests/PerfPerCostLogicTests.cs` — formula
  arithmetic, null propagation, and the `shared/modes.json` family drift guard.
- `tests/Networker.ControlPlane.Tests/CloudCostTableTests.cs` — curated-file
  validation + lookup semantics.
- `tests/Networker.ControlPlane.Tests/PerfPerCostContractTests.cs` — wire
  shape (snake_case field sets, null-cost honesty).
- `tests/Networker.Tests/PerfPerCostReportTests.cs` — end-to-end against real
  Postgres: 401/403/member-read authz, the missing-tester-schema empty
  report, and hand-computed aggregate/value-score numbers.
- `dashboard/src/pages/ValueReportPage.test.tsx` — table sorting,
  missing-cost banner, empty states, disclaimer/formula footer.
