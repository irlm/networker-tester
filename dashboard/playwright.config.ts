import { defineConfig, devices } from '@playwright/test';

/**
 * Audit P2: browser E2E.
 *
 * The vitest suite mounts components with `../api/client` mocked. That proves
 * component logic, but it never boots the real app — so it cannot see a broken
 * lazy chunk, a router misconfiguration, a crash inside an error boundary, or
 * a page that renders blank because a hook threw during the real mount. Those
 * are the failures a user actually meets first.
 *
 * These run against the PRODUCTION BUILD via `vite preview`, not the dev
 * server, so the bundle under test is the bundle that ships (code-split chunks
 * included — a lazy import that 404s in prod resolves fine in dev).
 *
 * The API is intercepted in-test rather than requiring a live control plane:
 * this suite is about the browser half of the stack. The frontend⇄server route
 * contract is covered separately by src/api/route-contract.test.ts.
 */
export default defineConfig({
  testDir: './e2e',
  // A failing E2E must be reproducible, not "sometimes". Retries hide flake;
  // on CI one retry absorbs genuine infrastructure blips only.
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  timeout: 30_000,
  expect: { timeout: 10_000 },

  use: {
    baseURL: 'http://127.0.0.1:4173',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },

  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],

  webServer: {
    // `vite preview` serves dist/, so `npm run build` must have run first.
    // --host 127.0.0.1 is load-bearing: `vite preview` otherwise binds only to
    // `localhost`, which resolves to ::1 on this toolchain, and the readiness
    // probe against 127.0.0.1 never connects (the server starts, Playwright
    // waits the full 120s and reports a timeout that looks like a build failure).
    command: 'npm run preview -- --port 4173 --strictPort --host 127.0.0.1',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
