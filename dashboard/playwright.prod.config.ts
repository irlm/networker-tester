import { defineConfig, devices } from '@playwright/test';

/**
 * Production UI smoke harness — post-deploy verification against the LIVE
 * dashboard (https://laghound.com by default), driven by scripts/prod-smoke.sh.
 *
 * Deliberately different from playwright.config.ts:
 *  - NO webServer: the target is an external base URL (PROD_SMOKE_BASE_URL),
 *    never a locally-served build.
 *  - Auth is a pre-minted JWT injected into localStorage before navigation
 *    (see e2e/prod/support.ts) — never a password login.
 *  - Every spec is read-only: render/navigation assertions only. Nothing in
 *    this suite launches runs, creates configs, deploys VMs, or mutates
 *    anything. See docs/prod-smoke.md.
 *
 * Env (sourced from the git-ignored .prod-smoke.env by scripts/prod-smoke.sh):
 *  - PROD_SMOKE_TOKEN         (required) platform JWT, injected into localStorage
 *  - PROD_SMOKE_PROJECT_ID    (required) project the read-only specs browse
 *  - PROD_SMOKE_BASE_URL      (optional) defaults to https://laghound.com
 *  - PROD_SMOKE_COMPARE_GROUP (optional) comparison-group id for the pivots spec
 */
export default defineConfig({
  testDir: './e2e/prod',
  // Keep raw Playwright debris (traces, retry context) out of the named
  // screenshots directory root.
  outputDir: './e2e/prod/artifacts/test-output',

  // A prod smoke failure must be reproducible and actionable — retries would
  // hide exactly the post-deploy flake this harness exists to catch.
  retries: 0,
  // Serial + single worker: be gentle to production.
  fullyParallel: false,
  workers: 1,

  reporter: [
    ['list'],
    // scripts/prod-smoke.sh parses this to build the per-spec GitHub issue.
    ['json', { outputFile: 'e2e/prod/artifacts/results.json' }],
  ],

  // Live pages over the real network: give lazy chunks + API round-trips room.
  timeout: 60_000,
  expect: { timeout: 15_000 },

  use: {
    baseURL: process.env.PROD_SMOKE_BASE_URL ?? 'https://laghound.com',
    trace: 'retain-on-failure',
    // support.ts captures a named full-page screenshot per spec instead.
    screenshot: 'off',
  },

  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1440, height: 900 },
      },
    },
  ],
});
