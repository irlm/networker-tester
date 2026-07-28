# Runbook: Diagnose a slow page with `perf_log`

The frontend and the control plane record per-call timing in the `perf_log`
table (shipped v0.28.61) in the `alethedash_core` database. Each API response
carries an `X-Process-Time-Ms` header (`ServerTiming.cs`). The frontend reads
this header. It splits each call into server time and network time before it
logs the call.

## Columns

| Column | Meaning |
|---|---|
| `kind` | `api` (a network call) or `render` (a client render) |
| `path` | API path or route rendered |
| `total_ms` | Total client-observed time |
| `server_ms` | Server processing (from `X-Process-Time-Ms`) — the server truth |
| `network_ms` | `total_ms − server_ms` (transport + client overhead) |
| `status` | HTTP status; `0` = the request never completed (aborted) |
| `source` | Where the entry came from |
| `component` | React component (for `render` rows) |
| `render_ms` | Render duration (for `render` rows) |
| `item_count` | Rows/items rendered |

`perf_log` excludes aborted requests, so they do not pollute the p95. But a row
with `status = 0` and NULL `server_ms` can still come from the client-side
instrumentation. See the interpretation note below.

## Key queries

p50/p95 by API path:

```sql
SELECT path,
       count(*)                                          AS n,
       percentile_cont(0.50) WITHIN GROUP (ORDER BY total_ms) AS p50,
       percentile_cont(0.95) WITHIN GROUP (ORDER BY total_ms) AS p95
FROM perf_log
WHERE kind = 'api' AND status > 0
GROUP BY path
ORDER BY p95 DESC;
```

Slowest API calls (server vs network split):

```sql
SELECT path, total_ms, server_ms, network_ms, status
FROM perf_log
WHERE kind = 'api' AND status > 0
ORDER BY total_ms DESC
LIMIT 50;
```

Render time by component:

```sql
SELECT component,
       percentile_cont(0.95) WITHIN GROUP (ORDER BY render_ms) AS p95_render,
       max(item_count) AS max_items
FROM perf_log
WHERE kind = 'render'
GROUP BY component
ORDER BY p95_render DESC;
```

## Interpretation lesson

A large `total_ms` with **NULL `server_ms` and `status = 0`** is an **aborted
poll**. This is a client-side artifact: a navigation or an unmount cancelled the
request. It is **not** server latency. Do not treat it as a slow endpoint. The
server truth is `server_ms`. Investigate a path only when its `server_ms` p95 is
high.
