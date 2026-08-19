import { expect, test, type Page } from '@playwright/test';

/**
 * Deep user flows on the production bundle — the third wave, closing the
 * 2026-08 test assessment's last gap. app.spec.ts proves the shell boots;
 * journeys.spec.ts pins the react-hooks behaviours; these pin the two flows
 * whose backends broke silently in production while the UI looked fine:
 *
 *   - the EXPORT journey (v0.28.96: DOCX export 500'd in prod while 285 unit
 *     tests were green — here the click-through path, format param, auth
 *     header, download event and error-toast surface are pinned in a real
 *     browser);
 *   - the MATRIX WIZARD launch (v0.28.110: matrix launch was an unimplemented
 *     stub returning 202 + zero runs — here the wizard's REQUEST SHAPE is
 *     pinned: cells composed from testbeds × languages, pending endpoints,
 *     the launch call, and the redirect).
 *
 * Stubs are shape-faithful to the client source (the app.spec.ts lesson).
 */

const PID = 'proj-e2e-001';

const IGNORED_CONSOLE = [
  /favicon/i,
  /Download the React DevTools/i,
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

/** Shape-faithful stubs for everything the Dashboard and wizard pages load. */
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
    // Cloud accounts: the wizard's testbed rows need one to be launchable
    // (step-1 canProceed requires every testbed to carry a cloudAccountId).
    if (path.endsWith('/cloud-accounts')) {
      return json([{
        account_id: 'acct-e2e-azure',
        name: 'Azure (e2e)',
        provider: 'azure',
        region_default: 'eastus',
        personal: false,
        status: 'active',
        last_validated: new Date(0).toISOString(),
      }]);
    }
    // GET /api/modes → { groups, language_capabilities: LanguageCapability[] }
    // (an ARRAY of rows, not a map — the first guess crashed the Languages
    // step into an error boundary; shape from api/client.ts getModes).
    if (path.endsWith('/api/modes')) {
      return json({
        groups: [],
        language_capabilities: [
          { language: 'rust', http1: true, http2: true, http3: true, apibench: true },
          { language: 'go', http1: true, http2: true, http3: false, apibench: true },
          { language: 'python', http1: true, http2: true, http3: false, apibench: true },
          { language: 'nodejs', http1: true, http2: true, http3: false, apibench: true },
        ],
      });
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

// ── Export journey ───────────────────────────────────────────────────────────

test.describe('report export journey', () => {
  test('export menu downloads with the right format, auth and filename', async ({ page }) => {
    const problems = watchForErrors(page);
    await signIn(page);

    let exportRequest: { format: string | null; auth: string | null } | null = null;
    await page.route('**/reports/integrated*', (route) => {
      const url = new URL(route.request().url());
      exportRequest = {
        format: url.searchParams.get('format'),
        auth: route.request().headers()['authorization'] ?? null,
      };
      return route.fulfill({
        status: 200,
        contentType: 'text/markdown',
        headers: {
          'Content-Disposition': 'attachment; filename="integrated-report-e2e.md"',
        },
        body: '# Integrated Test Report\n\ne2e stub body\n',
      });
    });

    await page.goto(`/projects/${PID}`);
    const exportButton = page.getByRole('button', { name: /Export report/ });
    await expect(exportButton).toBeVisible();
    await exportButton.click();

    const downloadPromise = page.waitForEvent('download');
    await page.getByRole('menuitem', { name: 'Markdown' }).click();
    const download = await downloadPromise;

    // The server-sent filename wins over the fileBase fallback.
    expect(download.suggestedFilename()).toBe('integrated-report-e2e.md');
    expect(exportRequest).not.toBeNull();
    expect(exportRequest!.format).toBe('md');
    expect(exportRequest!.auth).toBe('Bearer e2e-fake-token');
    expect(problems()).toEqual([]);
  });

  test('a failing export surfaces an error toast, not silence', async ({ page }) => {
    await signIn(page);
    await page.route('**/reports/integrated*', (route) =>
      route.fulfill({ status: 500, body: 'render exploded' }));

    await page.goto(`/projects/${PID}`);
    await page.getByRole('button', { name: /Export report/ }).click();
    await page.getByRole('menuitem', { name: 'Word (.docx)' }).click();

    // The v0.28.96 class: a 500 from one format must reach the user.
    await expect(page.getByText(/Export failed/)).toBeVisible({ timeout: 5_000 });
  });
});

// ── Matrix wizard journey ────────────────────────────────────────────────────

test.describe('application benchmark wizard', () => {
  test('composes cells from testbeds x languages and launches the group', async ({ page }) => {
    const problems = watchForErrors(page);
    await signIn(page);

    interface CreateBody {
      cells: Array<{ endpoint: { kind: string; language?: string } }>;
      base_workload: { modes: string[]; runs: number };
    }
    let createBody: CreateBody | null = null;
    let launched = false;
    await page.route('**/comparison-groups', async (route) => {
      createBody = route.request().postDataJSON();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ id: 'cg-e2e-0001', ...createBody }),
      });
    });
    await page.route('**/comparison-groups/cg-e2e-0001/launch', async (route) => {
      launched = true;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ launched: 2 }),
      });
    });
    // The post-launch compare page (#803) reads the group detail.
    await page.route('**/api/v2/comparison-groups/cg-e2e-0001', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: 'cg-e2e-0001',
          project_id: PID,
          name: 'Validation Run',
          base_workload: createBody?.base_workload ?? { modes: [], runs: 0 },
          cells: createBody?.cells ?? [],
          status: 'running',
          created_at: new Date().toISOString(),
          runs: [],
        }),
      }));

    await page.goto(`/projects/${PID}/benchmarks/application/new`);

    // Step 1 (Template): "Validation Run" applies rust+python and jumps to
    // Testbeds — two languages x one testbed = a MATRIX launch out of the box.
    await page.getByRole('button', { name: /Validation Run/ }).click();

    // Step 2 (Testbeds): the template's default row needs a cloud account —
    // pick the stubbed one via the combobox (focus opens the listbox).
    await page.getByRole('combobox').first().click();
    await page.getByRole('option', { name: /Azure \(e2e\)/ }).click();
    await page.getByRole('button', { name: /^Next/ }).click();
    // Step 3 (Languages): pre-selected by the template.
    await page.getByRole('button', { name: /^Next/ }).click();
    // Step 4 (Methodology): template-configured.
    await page.getByRole('button', { name: /^Next/ }).click();

    // Step 5 (Review): name defaults to the placeholder; launch.
    await page.getByRole('button', { name: /Launch \d+ Runs/ }).click();

    await expect
      .poll(() => launched, { timeout: 10_000, message: 'launch call never fired' })
      .toBe(true);

    // The v0.28.110 request-shape pin: cells composed as languages x testbeds,
    // each a PENDING endpoint carrying the language; matrix workload intact.
    expect(createBody).not.toBeNull();
    expect(createBody!.cells.length).toBeGreaterThanOrEqual(2);
    for (const cell of createBody!.cells) {
      expect(cell.endpoint.kind).toBe('pending');
      expect(['rust', 'python']).toContain(cell.endpoint.language);
    }
    expect(createBody!.base_workload.modes.length).toBeGreaterThan(0);
    expect(createBody!.base_workload.runs).toBeGreaterThan(0);

    // The group IS the experiment: launch lands on its compare page (#803),
    // which live-polls the cells — not on the filtered runs list.
    await expect(page).toHaveURL(/benchmarks\/compare\/cg-e2e-0001/);
    await expect(page.getByRole('heading', { name: 'Validation Run' })).toBeVisible();
    expect(problems()).toEqual([]);
  });
});
