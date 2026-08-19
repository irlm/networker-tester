import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test as base, expect, type APIRequestContext, type Page } from '@playwright/test';

/**
 * Shared plumbing for the prod smoke specs (playwright.prod.config.ts).
 *
 * READ-ONLY GUARANTEE: nothing in this file or the specs that use it launches
 * runs, creates configs, deploys VMs, or mutates production state. The only
 * API calls made outside the browser are GETs (run discovery + platform-admin
 * detection), and the only in-page interactions are navigation, tab/combobox
 * opening, and purely client-side wizard-state clicks.
 *
 * Auth: the SPA keeps its session in localStorage (see src/api/http.ts
 * clearSession + src/stores/authStore.ts) — the JWT lives under the `token`
 * key and is sent as `Authorization: Bearer <token>`. We inject that plus the
 * session-shape keys the boot path reads (`email`, `role`, `status`,
 * `isPlatformAdmin`, `activeProjectId`) via addInitScript BEFORE any page
 * script runs, so the app boots straight into the authenticated shell.
 * Never a password login.
 */

export const BASE_URL = process.env.PROD_SMOKE_BASE_URL ?? 'https://laghound.com';
export const COMPARE_GROUP = process.env.PROD_SMOKE_COMPARE_GROUP ?? '';

const ARTIFACTS_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'artifacts');

export function requireToken(): string {
  const token = process.env.PROD_SMOKE_TOKEN;
  if (!token) {
    throw new Error(
      'PROD_SMOKE_TOKEN is not set. Run through scripts/prod-smoke.sh (which sources .prod-smoke.env), or see docs/prod-smoke.md for setup.',
    );
  }
  return token;
}

export function requireProjectId(): string {
  const pid = process.env.PROD_SMOKE_PROJECT_ID;
  if (!pid) {
    throw new Error(
      'PROD_SMOKE_PROJECT_ID is not set. Add it to .prod-smoke.env (see docs/prod-smoke.md).',
    );
  }
  return pid;
}

/** Route inside the smoke project, e.g. projectPath('/runs'). */
export function projectPath(suffix: string): string {
  return `/projects/${requireProjectId()}${suffix}`;
}

export function authHeaders(): Record<string, string> {
  return { Authorization: `Bearer ${requireToken()}` };
}

/** Authenticated read-only GET against the prod API. */
export async function apiGet<T>(request: APIRequestContext, apiPath: string): Promise<T> {
  const res = await request.get(`${BASE_URL}/api${apiPath}`, { headers: authHeaders() });
  if (!res.ok()) {
    throw new Error(`GET /api${apiPath} failed: ${res.status()} ${res.statusText()}`);
  }
  return (await res.json()) as T;
}

// Platform-admin detection: GET /api/admin/canary is a read-only status
// endpoint gated to platform admins — 200 means the token can see the admin
// surface (canary panel spec), anything else means member-level smoke only.
// Cached across tests: the answer cannot change mid-suite.
let platformAdminCache: Promise<boolean> | null = null;
export function detectPlatformAdmin(request: APIRequestContext): Promise<boolean> {
  platformAdminCache ??= request
    .get(`${BASE_URL}/api/admin/canary`, { headers: authHeaders() })
    .then((res) => res.ok())
    .catch(() => false);
  return platformAdminCache;
}

// Console noise that must not fail a spec: the live-updates WebSocket can
// drop/reconnect at any moment (the UI has a banner for exactly that), and
// Chrome logs the failed upgrade as a console error.
const ALLOWED_CONSOLE_PATTERNS: RegExp[] = [
  /websocket/i,
  /wss?:\/\//i,
  /live updates/i,
  // Chrome aborts in-flight requests (most visibly the always-open SSE
  // stream) whenever a LOCAL network interface changes — Docker bridge/veth
  // churn on the machine running this harness triggers it constantly. The
  // app auto-reconnects; failing a prod spec on it would be a false alarm.
  /ERR_NETWORK_CHANGED/,
  /\/api\/events\/approval/,
];

function slugify(title: string): string {
  return title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
}

/** Where the named full-page screenshot for a spec title lands. */
export function screenshotPathFor(title: string): string {
  return path.join(ARTIFACTS_DIR, `${slugify(title)}.png`);
}

interface ProdFixtures {
  /** Uncaught page errors + non-allowlisted console errors, asserted empty at teardown. */
  collectedErrors: string[];
}

export const test = base.extend<ProdFixtures>({
  // eslint-disable-next-line no-empty-pattern -- Playwright fixtures require the destructuring pattern
  collectedErrors: async ({}, provide) => {
    await provide([]);
  },

  page: async ({ page, request, collectedErrors }, provide, testInfo) => {
    const token = requireToken();
    const projectId = requireProjectId();
    const platformAdmin = await detectPlatformAdmin(request);

    // Session injection — runs before any page script on every navigation.
    await page.addInitScript(
      (session: { token: string; projectId: string; admin: string }) => {
        localStorage.setItem('token', session.token);
        localStorage.setItem('email', 'prod-smoke@harness.local');
        localStorage.setItem('role', session.admin === 'true' ? 'admin' : 'member');
        localStorage.setItem('status', 'active');
        localStorage.setItem('isPlatformAdmin', session.admin);
        localStorage.setItem('activeProjectId', session.projectId);
      },
      { token, projectId, admin: platformAdmin ? 'true' : 'false' },
    );

    // Console-error collection: any uncaught page error or console.error that
    // is not allow-listed WebSocket churn fails the spec at teardown.
    page.on('pageerror', (err) => {
      collectedErrors.push(`pageerror: ${err.message}`);
    });
    page.on('console', (msg) => {
      if (msg.type() !== 'error') return;
      const text = msg.text();
      if (ALLOWED_CONSOLE_PATTERNS.some((re) => re.test(text))) return;
      collectedErrors.push(`console.error: ${text}`);
    });

    await provide(page);

    // Full-page screenshot for every spec, pass or fail — named after the
    // test title so scripts/prod-smoke.sh can attach it to the failure issue.
    if (!page.isClosed()) {
      await page
        .screenshot({ fullPage: true, path: screenshotPathFor(testInfo.title) })
        .catch(() => {
          /* page may be mid-teardown; the trace still has the failure state */
        });
    }

    expect(collectedErrors, 'uncaught page errors / console errors on the live page').toEqual([]);
  },
});

export { expect };

/** Navigate within the app (baseURL-relative) and let the SPA boot. */
export async function gotoApp(page: Page, pathname: string): Promise<void> {
  await page.goto(pathname, { waitUntil: 'domcontentloaded' });
}

/**
 * The two ways the shell surfaces a broken page without throwing: the React
 * error boundary ("something broke") and an error toast. Assert neither is up.
 */
export async function expectNoErrorUI(page: Page): Promise<void> {
  await expect(page.getByText('something broke')).toHaveCount(0);
  // Error toasts: Toast.tsx renders `toast-enter` + the red error palette.
  await expect(page.locator('[role="alert"].toast-enter.text-red-400')).toHaveCount(0);
}

// ── Read-only run discovery (spec 8) ──────────────────────────────────────

export interface SmokeRun {
  id: string;
  status: 'queued' | 'provisioning' | 'running' | 'completed' | 'failed' | 'cancelled';
  error_message: string | null;
  success_count: number;
  failure_count: number;
}

/** Newest runs first (the list endpoint returns newest-first, capped). */
export async function listRecentRuns(request: APIRequestContext): Promise<SmokeRun[]> {
  return apiGet<SmokeRun[]>(request, `/v2/projects/${requireProjectId()}/test-runs?limit=50`);
}
