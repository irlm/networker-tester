# URL comparison report

**"Of the URLs I watch, which is fastest, which flakes, and which is
steadiest?"** — and, just as importantly, **"may I believe that answer?"**

Phase 3 of [#782](https://github.com/irlm/networker-tester/issues/782). P1 made
it possible to probe several URLs in one run; P2 made each point a burst of
samples with a real median and spread. This report is what that data was for.

- API: `GET /api/projects/{projectId}/reports/probe-comparison` (member-read —
  any project role, enforced by the `ProjectMember` policy).
- Page: `/projects/{projectId}/probe/compare` — reachable from the URL Probe
  watchlist's **Compare…** action once two or more rows are ticked.
- Code: `src/Networker.ControlPlane/Endpoints/ProbeComparisonEndpoints.cs`
  (SQL + wire shape), `src/Networker.ControlPlane/Reports/ProbeComparisonLogic.cs`
  (the methodology, unit-tested without a DB),
  `src/Networker.ControlPlane/Reports/Documents/ProbeComparisonReportDocument.cs`
  (PDF/HTML/DOCX/Markdown export), `dashboard/src/pages/ProbeComparePage.tsx`.

## The methodology, and why it is not "aggregate each URL over the window"

A raw window-wide aggregate is **biased by the probing schedule**. A URL probed
only overnight wins by dodging peak hours, and nothing in the number says so.
Every figure in this report is therefore computed over **shared time buckets** —
buckets in which *every* compared URL actually has measurements.

| Step | Rule |
| --- | --- |
| 1. Bucket | Attempts are grouped into fixed-width buckets (`15m` / `1h` / `6h` / `1d`) by start time. |
| 2. Qualify | A bucket counts for a URL once it holds at least `min_samples` (default **3**) samples of it. One sample is an anecdote; burst sampling makes 5 per probe the norm on this surface. |
| 3. Eligibility | A URL whose own qualifying buckets cover **less than 30%** of the window is **excluded** — shown greyed with its real counts, never ranked, and never allowed to shrink everyone else's shared set. |
| 4. Intersect | The **shared** set is the intersection of the eligible URLs' qualifying buckets. Every headline number comes from it and nothing else. |
| 5. Rank | The scoreboard is ranked only when the shared set covers **≥ 30%** of the window *and* holds **≥ 5** buckets. Below either floor the report says "insufficient overlap" and greys the ranking rather than ranking anyway. |

**Ranking is head-to-head**, not "whose average is lower": for each pair, the
number of shared buckets in which one URL's p50 beat the other's. *"In the 42
hours both were probed, A was faster in 31"* survives an outlier that a mean
does not, and it is the claim the report actually makes.

Every per-URL figure is a **median of that URL's per-bucket values**, so each
bucket weighs the same however many times the URL happened to be probed inside
it — a URL probed twice an hour must not outvote one probed once.

### What the report will not do

- **Never mixes modes.** An http1 probe and an http3 probe of the same URL are
  not the same race, so the report is computed once per mode and the response
  carries one scoreboard per mode. The page shows a tab per mode.
- **Never interpolates.** A bucket a URL did not measure is absent, not zero. On
  the chart that is a gap in the line, deliberately (`connectNulls={false}`).
- **Never awards a crown on a tie**, and never lets a phase nothing measured win
  one — "no TLS handshake" is not "the best TLS". Both serialise as `null`.
- **Never counts a one-sided bucket as a win.** If either side had no successful
  sample in a bucket, that bucket is not a race and is counted in no column.

## Crowns

| Crown | Metric |
| --- | --- |
| Fastest | Lowest median of the per-bucket p50s |
| Most reliable | Highest share of samples that succeeded (with the dominant `ErrorCategory` alongside) |
| Most consistent | Lowest `p95 / p50` — whose median best predicts any single request |
| Best DNS / TCP / TLS / TTFB | Lowest median for that phase — often three different URLs, which is the insight |

## Where the numbers come from

The report reads the tester-owned V001 probe schema directly (raw Npgsql, like
the app-network and perf-per-cost reports), joined to the control plane's own
`test_run` / `test_config`:

| Quantity | Source |
| --- | --- |
| URL identity | `RequestAttempt.TargetUrl` (V006), falling back to the tester `TestRun.TargetUrl` — attempts written before v0.28.231 and single-target runs of that era have no per-attempt URL, and without the fallback the whole pre-#820 history of single-URL probes would be invisible here |
| `total_ms` | `HttpResult.TotalDurationMs`, else the attempt's wall time |
| phases | `DnsResult.DurationMs`, `TcpResult.ConnectDurationMs`, `TlsResult.HandshakeDurationMs`, `HttpResult.TtfbMs` |
| errors | `ErrorRecord.ErrorCategory`, aggregated separately (it is one-to-many per attempt; folding it into the main aggregate would multiply the sample counts) |

Runs are restricted to `test_config.test_kind = 'url_probe'`, so benchmark,
canary and deployment runs can never drift into a URL comparison. Project
isolation is enforced **in SQL** (`test_config.project_id = $1`), not by a
launch-time invariant. A missing tester schema (`42P01`) yields an empty, valid
report — never an error.

**Bucketing is `floor(epoch / width)`, not TimescaleDB's `time_bucket`.** The
results are identical for the fixed, epoch-aligned widths this report offers;
the arithmetic form adds no extension dependency (so it also works against a
plain-PostgreSQL test or lab database) and expresses the `15m` and `6h` widths
that `date_trunc` cannot.

## Query parameters

| Parameter | Values | Default |
| --- | --- | --- |
| `urls` | comma-separated, 2–8 URLs; trimmed, de-duplicated, order preserved | — |
| `window` | `24h`, `7d`, `30d` | `7d` |
| `bucket` | `15m`, `1h`, `6h`, `1d` | `1h` |
| `min_samples` | 1–50 | `3` |
| `format` | `pdf`, `html`, `docx`, `md` (omit for JSON) | JSON |

With **fewer than two** URLs the response is a **discovery call**: `available`
lists every URL with probe data in the window and its sample count, and `modes`
is empty. The compare page populates its picker from exactly that, so the picker
and the report can never disagree about what a URL is called.

The **Compare…** action on the watchlist passes URLs through the same
`toProbeUrl()` normalisation the probe launch uses, so the strings it sends are
byte-identical to the `target_url` the tester stamps on each attempt.

## Coverage honesty on screen

The shared coverage (`42 / 168 buckets · 25%`) is rendered **above** the
scoreboard, not below it — it is the licence to read everything under it. When a
mode is not ranked the panel turns amber, the crowns disappear entirely, the
scoreboard is dimmed, and the reason names the real numbers and what to do about
it (probe them on the same schedule). Excluded URLs are listed by name with the
buckets they do have.

An exported document carries the same coverage block, the same "not ranked"
callout and the full methodology — a scoreboard that leaves the building without
them is a claim nobody can check.

## See also

- [`reports-export.md`](reports-export.md) — the PDF/HTML/DOCX/Markdown pipeline
- [`reports-app-network.md`](reports-app-network.md) — the other probe-schema report
- [`probes.md`](probes.md) — the URL Probe surface these measurements come from
