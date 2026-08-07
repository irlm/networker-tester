import { expect, test, type Page } from '@playwright/test';

/**
 * User-journey specs on the production bundle — the second wave after
 * app.spec.ts proved the shell boots.
 *
 * These pin the BEHAVIOURAL guarantees from the react-hooks refactor
 * (v0.28.163), which until now were only verified by hand in a browser:
 *
 *   - `?modes=` seeds the initial selection via a lazy useState initialiser.
 *     The old effect-based prefill rendered the defaults first and then
 *     replaced them — these specs fail if that regresses to a blank or
 *     default selection.
 *   - a refetch keeps current data visible instead of blanking to a spinner
 *     (the removed synchronous `setLoading(true)`).
 *
 * API stubs are SHAPE-FAITHFUL, verified against the client source, because
 * the first version of app.spec.ts crashed a page by stubbing `/api/projects`
 * as a bare array when the client reads `{ projects: [...] }`.
 */

const PID = 'proj-e2e-001';

const IGNORED_CONSOLE = [
  /favicon/i,
  /Download the React DevTools/i,
  // vite preview has no /ws to upgrade — harness property, not an app defect.
  /WebSocket connection to /i,
];

function watchForErrors(page: Page): () => string[] {
  const problems: string[] = [];
  page.on('console', (msg) => {
    if (msg.type() !== 'error') return;
    const text = msg.text();
    if (IGNORED_CONSOLE.some((rx) => rx.test(text))) return;
    problems.push(`console.error: ${text}`);
  });
  page.on('pageerror', (err) => problems.push(`pageerror: ${err.message}`));
  return () => problems;
}

/** Two lifecycle rows, shaped per VmLifecycleRow (api/vmHistory.ts). */
function vmHistoryRows() {
  const base = {
    project_id: PID,
    resource_name: 'eastus-runner-1',
    cloud: 'azure',
    region: 'eastus',
    vm_size: 'Standard_B2s',
    vm_name: 'tester-eastus-e2e',
    vm_resource_id: null,
    cloud_connection_id: null,
    cloud_account_name_at_event: 'Azure (credits)',
    provider_account_id: null,
    triggered_by: null,
    metadata: null,
  };
  return [
    {
      ...base,
      event_id: 'e2e-ev-1',
      resource_type: 'tester',
      resource_id: 'e2e-tester-1',
      event_type: 'created',
      event_time: '2026-08-01T10:00:00Z',
      created_at: '2026-08-01T10:00:00Z',
    },
    {
      ...base,
      event_id: 'e2e-ev-2',
      resource_type: 'tester',
      resource_id: 'e2e-tester-1',
      event_type: 'started',
      event_time: '2026-08-01T10:01:00Z',
      created_at: '2026-08-01T10:01:00Z',
    },
  ];
}

async function stubApi(page: Page) {
  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        headers: { 'X-Process-Time-Ms': '1.0' },
        body: JSON.stringify(body),
      });

    if (path.endsWith('/api/auth/profile')) {
      return json({
        user_id: '11111111-1111-4111-8111-111111111111',
        email: 'e2e@example.com',
        role: 'admin',
        status: 'active',
        is_platform_admin: true,
        must_change_password: false,
      });
    }
    if (path.endsWith('/api/auth/sso/providers')) return json({ providers: [] });
    if (path.endsWith('/api/projects')) {
      return json({
        projects: [{
          project_id: PID,
          name: 'E2E Project',
          slug: 'e2e-project',
          description: null,
          created_at: new Date(0).toISOString(),
          updated_at: new Date(0).toISOString(),
          role: 'admin',
        }],
      });
    }
    // VmHistoryResponse: { events, has_more } — NOT a bare array.
    if (path.includes('/vm-history')) {
      return json({ events: vmHistoryRows(), has_more: false });
    }
    return json([]);
  });
}

async function signIn(page: Page) {
  await stubApi(page);
  await page.addInitScript(() => {
    localStorage.setItem('token', 'e2e-fake-token');
    localStorage.setItem('email', 'e2e@example.com');
    localStorage.setItem('role', 'admin');
    localStorage.setItem('status', 'active');
  });
}

test.describe('URL-seeded state (the lazy-initialiser refactor)', () => {
  test('?modes= seeds the network-test selection on first paint', async ({ page }) => {
    const problems = watchForErrors(page);
    await signIn(page);

    await page.goto(`/projects/${PID}/tests/new?modes=tcp,dns`);

    // The page's own summary line is the user-visible truth. Under the old
    // effect-based prefill this read "pick at least one mode" for a frame and
    // could regress to staying that way.
    await expect(page.getByText('2 modes selected')).toBeVisible({ timeout: 15_000 });
    expect(problems(), problems().join('\n')).toEqual([]);
  });

  test('unknown modes in the URL are dropped, valid ones kept', async ({ page }) => {
    await signIn(page);

    await page.goto(`/projects/${PID}/tests/new?modes=tcp,definitely-not-a-mode`);

    // Filtering happens in the initialiser (`ALL_MODES.has`) — a bogus mode
    // must not produce a phantom selection or crash the launch payload later.
    await expect(page.getByText('1 mode selected')).toBeVisible({ timeout: 15_000 });
  });

  test('no ?modes= means an empty selection, not a crash', async ({ page }) => {
    await signIn(page);
    await page.goto(`/projects/${PID}/tests/new`);
    await expect(page.getByText('pick at least one mode')).toBeVisible({ timeout: 15_000 });
  });
});

test.describe('refetch keeps data visible (the setLoading refactor)', () => {
  test('VM history rows survive a filter change without blanking', async ({ page }) => {
    const problems = watchForErrors(page);
    await signIn(page);

    await page.goto(`/projects/${PID}/vms/history`);
    await expect(page.getByText('tester-eastus-e2e').first()).toBeVisible({ timeout: 15_000 });

    const rows = () => page.locator('tbody tr').count();
    expect(await rows()).toBe(2);

    // Change the type filter — this refetches. The old code set loading=true
    // synchronously and blanked the table; the rows must stay put now.
    await page.getByRole('button', { name: /^Runner/ }).click();

    // Sample IMMEDIATELY — this is the window where the blank used to happen.
    // No waiting: if the table is empty on the very next read, the regression
    // is back.
    expect(await rows(), 'rows blanked during refetch').toBe(2);

    // …and after the refetch settles they are still there.
    await expect(page.getByText('tester-eastus-e2e').first()).toBeVisible();
    expect(await rows()).toBe(2);
    expect(problems(), problems().join('\n')).toEqual([]);
  });

  test('a failing refetch shows the error WITHOUT discarding the rows', async ({ page }) => {
    await signIn(page);
    await page.goto(`/projects/${PID}/vms/history`);
    await expect(page.getByText('tester-eastus-e2e').first()).toBeVisible({ timeout: 15_000 });

    // From here on, the API fails — rows are already on screen.
    await page.unroute('**/api/**');
    await page.route('**/api/**', (route) =>
      route.fulfill({ status: 502, contentType: 'text/plain', body: 'Bad Gateway' }));

    await page.getByRole('button', { name: 'Refresh' }).click();

    // The error banner appears…
    await expect(
      page.getByText(/Server unavailable|Failed to load/i).first(),
    ).toBeVisible({ timeout: 15_000 });
    // …and the stale rows are STILL visible alongside it. The old code
    // cleared the error optimistically and blanked the data before knowing
    // whether the request would succeed.
    expect(await page.locator('tbody tr').count(), 'rows discarded on error').toBe(2);
  });
});
