# Prod UI smoke harness

Automated post-deploy verification of the live dashboard (laghound.com):
headless Playwright specs that render the key surfaces, screenshot each one,
and file a GitHub issue per failed spec — replacing the manual
click-through-prod ritual after a release.

- Suite: `dashboard/e2e/prod/*.prod.spec.ts` (config: `dashboard/playwright.prod.config.ts`)
- Runner: `scripts/prod-smoke.sh`
- Artifacts: `dashboard/e2e/prod/artifacts/` (git-ignored — full-page PNG per
  spec, `results.json`, traces under `test-output/`)

## The never-mutates guarantee

Every spec is **read-only**. The harness renders pages, switches client-side
tabs/pivots, opens a combobox, and follows navigation links. It **never**
launches runs or probes, creates or edits configs, deploys or touches VMs,
dispatches the canary, or submits any form. The only non-browser API calls are
authenticated `GET`s (run discovery + platform-admin detection). If a future
spec needs anything beyond that, it does not belong in this suite.

## Setup (one time per machine)

1. Sign in to the dashboard (the base URL you intend to smoke) with the
   account the harness should use. Platform-admin accounts additionally cover
   the `/admin/canary` spec; member accounts skip it with a note.
2. The SPA keeps its JWT in `localStorage` under the `token` key
   (`dashboard/src/api/http.ts`). With the app open, run this in the browser
   devtools console — it copies a ready-to-paste env file to the clipboard:

   ```js
   copy(`PROD_SMOKE_TOKEN=${localStorage.getItem('token')}\nPROD_SMOKE_PROJECT_ID=${localStorage.getItem('activeProjectId')}`)
   ```

3. Paste into `.prod-smoke.env` at the repo root (git-ignored — never commit
   it):

   ```sh
   PROD_SMOKE_TOKEN=eyJhbGciOi...            # required — JWT from localStorage
   PROD_SMOKE_PROJECT_ID=<project uuid>      # required — project the specs browse
   PROD_SMOKE_BASE_URL=https://laghound.com  # optional (this is the default)
   PROD_SMOKE_COMPARE_GROUP=<group uuid>     # optional — enables the comparison-pivots spec
   ```

   `PROD_SMOKE_COMPARE_GROUP` is a known comparison-group id (grab one from a
   `/benchmarks/compare/<id>` URL); without it that spec is skipped with a note.

4. Playwright browsers, if not already present:
   `cd dashboard && npx playwright install chromium`.

Tokens expire — when the suite starts failing with redirects to `/login`,
re-mint via step 2.

## Usage

```sh
./scripts/prod-smoke.sh
```

- **Green**: one summary line pointing at the artifacts dir; exit 0.
- **Red**: exit 1, and for each failed spec the runner
  - uploads its full-page screenshot as a **secret** gist (base64-encoded PNG —
    gists are text-only; decode with `base64 -d`),
  - checks `gh issue list --search` for an open issue with the same spec name
    in the title and **comments on it** instead of duplicating,
  - otherwise creates an issue titled
    `prod smoke failure: <spec name> (v<version> from /api/health)` with the
    gist link, the assertion error, the deployed version, and a triage
    checklist.

Requires `gh` (authenticated) and `curl` on the failure path.

Enumerate the suite without touching prod:
`cd dashboard && npx playwright test --config playwright.prod.config.ts --list`.

## How auth works

No password ever touches the harness. Before any navigation,
`dashboard/e2e/prod/support.ts` injects the pre-minted JWT and the
session-shape keys the app boots from (`token`, `email`, `role`, `status`,
`isPlatformAdmin`, `activeProjectId`) into `localStorage` via
`addInitScript`, exactly mirroring what `authStore.login()` writes. Platform
admin-ness is detected with a read-only `GET /api/admin/canary` so the
admin-only spec runs only when the token can actually see that surface.

## What it covers

| Spec file | Asserts |
|---|---|
| `runs.prod.spec.ts` | Runs table + purpose tabs (All / Network tests / URL probes / SDK probes / Benchmarks) render; tab switching updates the list with zero console/page errors; run detail of the newest finished run shows header, status chip, and attempts — plus the #795 guard: a failed run renders its `error_message` banner |
| `probe.prod.spec.ts` | URL Probe page: input visible, watchlist ("Watched URLs (N)") present |
| `scenarios.prod.spec.ts` | Start-a-test page: scenario cards render, every Configure link carries a project-scoped href, and one URL-probe card navigates end-to-end to `/probe?preset=quick` |
| `wizards.prod.spec.ts` | Full Stack wizard Testbeds step: cloud-account combobox opens and the account list loads (no selection); Application wizard via `?template=api-compute` lands on Testbeds with the "select a cloud account to continue" gate |
| `compare.prod.spec.ts` | Comparison-group pivots (By testbed / By language) render with per-cell `run <id> →` links — needs `PROD_SMOKE_COMPARE_GROUP`, skipped otherwise |
| `admin.prod.spec.ts` | `/admin/canary`: Trigger panel, Dispatch history, and Recent runs on GitHub sections render (the dispatch button is never clicked) — platform-admin token only |
| `system.prod.spec.ts` | Settings page: "system versions" panel with a version, "deployed targets" list (rows or its empty state) |

Cross-cutting, enforced by the shared fixture: every spec fails on any
uncaught page error or non-allow-listed `console.error` (WebSocket
reconnect churn is allow-listed — the UI has a banner for it by design), and
every spec leaves a named full-page screenshot in the artifacts dir, pass or
fail.
