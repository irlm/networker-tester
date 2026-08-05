import { expect, test, type Page } from '@playwright/test';

/**
 * Audit P2: the first browser E2E coverage this app has had.
 *
 * Every existing frontend test mounts components with `../api/client` mocked.
 * That is fine for component logic and useless for the failures a user meets
 * first: a lazy chunk that 404s in the production bundle, a router
 * misconfiguration, a hook that throws during the real mount and leaves a
 * blank page, an error boundary swallowing a crash. None of those can happen
 * in a jsdom test that never loads the built bundle.
 *
 * So these run against `vite preview` serving the REAL production build, with
 * the API intercepted at the network layer.
 */

/** Console errors that are environmental noise rather than app defects. */
const IGNORED_CONSOLE = [
  /favicon/i,
  /Failed to load resource.*404.*favicon/i,
  // React DevTools nag in production builds.
  /Download the React DevTools/i,
  // `vite preview` serves static files only — it has no /ws/dashboard to
  // upgrade, so the event-bus socket always fails here. That is a property of
  // the harness, not of the app; the real socket is covered by
  // RawWebSocketIntegrationTests against the actual control plane.
  /WebSocket connection to /i,
];

/**
 * Collect real page failures. Returns a getter rather than an array so a test
 * reads the state AFTER navigation instead of capturing an empty snapshot —
 * a mistake that makes this whole guard vacuous.
 */
function watchForErrors(page: Page): () => string[] {
  const problems: string[] = [];

  page.on('console', (msg) => {
    if (msg.type() !== 'error') return;
    const text = msg.text();
    if (IGNORED_CONSOLE.some((rx) => rx.test(text))) return;
    problems.push(`console.error: ${text}`);
  });

  // An uncaught exception never reaches console.error in some builds.
  page.on('pageerror', (err) => problems.push(`pageerror: ${err.message}`));

  // A 404 on a code-split chunk is the classic prod-only failure.
  page.on('response', (res) => {
    const url = res.url();
    if (res.status() >= 400 && /\.(js|css)(\?|$)/.test(url)) {
      problems.push(`asset ${res.status()}: ${url}`);
    }
  });

  return () => problems;
}

/**
 * Intercept every API call. Needed even for the LOGGED-OUT login page: it
 * queries /api/auth/sso/providers on mount, and `vite preview` proxies /api to
 * a control plane that isn't running, so the request 502s and lands in the
 * console-error collector.
 */
async function stubApi(page: Page) {
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;

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

    if (path.endsWith('/api/projects')) {
      // The client reads `.projects`, not a bare array (api/client.ts) — a bare
      // array is what crashed the page inside its error boundary the first time
      // this suite ran.
      return json({ projects: [
        {
          project_id: 'proj-e2e-001',
          name: 'E2E Project',
          slug: 'e2e-project',
          description: 'seeded by the browser E2E',
          created_at: new Date(0).toISOString(),
          updated_at: new Date(0).toISOString(),
          role: 'admin',
        },
      ] });
    }

    if (path.endsWith('/api/auth/sso/providers')) {
      return json({ providers: [] });
    }

    // Everything else: a shape-neutral empty response. The point of this suite
    // is that the app RENDERS, not that each payload is right — payload shape
    // is the route-contract test's job.
    return json([]);
  });
}

/** Stub the API and seed an authenticated session. */
async function signIn(page: Page) {
  await stubApi(page);
  await page.addInitScript(() => {
    localStorage.setItem('token', 'e2e-fake-token');
    localStorage.setItem('email', 'e2e@example.com');
    localStorage.setItem('role', 'admin');
    localStorage.setItem('status', 'active');
  });
}

test.describe('production bundle boots', () => {
  test('the login page renders its form', async ({ page }) => {
    const problems = watchForErrors(page);
    await stubApi(page);

    await page.goto('/login');

    // Assert on real, user-visible controls — "the page responded 200" would
    // pass on a blank white screen.
    // The form is email-FIRST: the password field only mounts after the email
    // step resolves (SSO accounts never see it). Asserting a password input up
    // front fails against the real app — exactly the kind of thing a
    // mocked-component test cannot tell you.
    await expect(page.getByPlaceholder('you@company.com')).toBeVisible();
    await expect(page.getByRole('button', { name: /continue|sign in/i })).toBeVisible();

    expect(problems(), `login page reported: ${problems().join('\n')}`).toEqual([]);
  });

  test('an authenticated route renders the app shell', async ({ page }) => {
    const problems = watchForErrors(page);
    await signIn(page);

    await page.goto('/projects');

    // The seeded project must actually appear: this proves the bundle mounted,
    // the router resolved, the lazy chunk loaded and data flowed through the
    // real client — none of which a mocked-module test exercises.
    // .first(): the name legitimately appears more than once (list row plus the
    // project switcher), and strict mode fails on multiple matches.
    await expect(page.getByText('E2E Project').first()).toBeVisible({ timeout: 15_000 });

    expect(problems(), `projects page reported: ${problems().join('\n')}`).toEqual([]);
  });

  test('a deep link survives a hard load', async ({ page }) => {
    // Client-side routes must be served by the SPA fallback. This is the
    // failure the nginx `try_files … /index.html` line exists to prevent, and
    // it only shows up on a fresh load of a nested path — never on in-app
    // navigation.
    await signIn(page);

    const response = await page.goto('/projects/proj-e2e-001');

    // The property under test is the SPA FALLBACK: a nested path that exists
    // only client-side must still be served index.html and mount the app.
    // Console-error freedom is deliberately NOT asserted here — this suite
    // stubs payloads shape-neutrally, so a detail page can legitimately
    // complain about missing data. The pages whose payloads ARE stubbed
    // faithfully carry that assertion instead.
    expect(response?.status(), 'deep link was not served by the SPA fallback').toBe(200);
    await expect(page.locator('#root')).not.toBeEmpty();
  });

  test('an unauthenticated deep link redirects to login', async ({ page }) => {
    // No token seeded: the guard must send the user to /login rather than
    // rendering a broken authenticated shell.
    await page.goto('/projects/proj-e2e-001');
    await expect(page).toHaveURL(/\/login/, { timeout: 15_000 });
  });

  test('the error watcher is not vacuous', async ({ page }) => {
    // Guards the guard: if the collector silently stopped seeing problems,
    // every assertion above would pass on a broken page. Prove it catches a
    // deliberate error.
    const problems = watchForErrors(page);
    await stubApi(page);
    await page.goto('/login');
    await page.evaluate(() => {
      console.error('deliberate probe error');
    });
    await expect.poll(() => problems().length).toBeGreaterThan(0);
  });
});
