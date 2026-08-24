import { test, expect, gotoApp, projectPath, expectNoErrorUI, apiGet } from './support';

/**
 * Every authenticated route, on production, must render its own shell — not an
 * error boundary and not a blank page.
 *
 * READ-ONLY: navigation only. No clicks that launch, create, deploy or mutate.
 *
 * The other prod specs assert deep content on a handful of surfaces; this one
 * trades depth for breadth, because "does every page still work after a
 * release" is a different question from "does the runs page show a status
 * chip". A route that 404s its lazy chunk, throws during mount, or loses its
 * router entry fails here and nowhere else.
 */

// Routes with no parameters beyond the project id.
const STATIC_ROUTES: ReadonlyArray<readonly [string, string]> = [
  ['dashboard', ''],
  ['scenarios', '/scenarios'],
  ['runs', '/runs'],
  ['runs compare', '/runs/compare'],
  ['probe', '/probe'],
  ['probe compare (#782 P3)', '/probe/compare'],
  ['network test', '/tests/new'],
  ['infrastructure', '/vms'],
  ['infrastructure history', '/vms/history'],
  ['sdk endpoints', '/sdk-endpoints'],
  ['schedules', '/schedules'],
  ['alerts', '/alerts'],
  ['members', '/members'],
  ['settings', '/settings'],
  ['share links', '/share-links'],
  ['approvals', '/approvals'],
  ['cloud accounts', '/cloud-accounts'],
  ['tls profiles', '/tls-profiles'],
  ['benchmarks', '/benchmarks'],
  ['benchmark catalog', '/benchmark-catalog'],
  ['benchmark regressions', '/benchmark-regressions'],
  ['benchmark wizard', '/benchmark-wizard'],
  ['app benchmark wizard', '/app-benchmark-wizard'],
  ['full-stack new', '/benchmarks/full-stack/new'],
  ['application new', '/benchmarks/application/new'],
  ['value report', '/reports/value'],
  ['app-network report', '/reports/app-network'],
];

const GLOBAL_ROUTES: ReadonlyArray<readonly [string, string]> = [
  ['projects', '/projects'],
  ['leaderboard', '/leaderboard'],
  ['bench tokens', '/bench-tokens'],
  ['bench token history', '/bench-tokens/history'],
  ['users', '/users'],
  ['admin system', '/admin/system'],
  ['admin perf log', '/admin/perf-log'],
];

test.describe('every production route renders', () => {
  for (const [name, suffix] of STATIC_ROUTES) {
    test(`project route: ${name}`, async ({ page }) => {
      const consoleErrors: string[] = [];
      page.on('pageerror', (e) => consoleErrors.push(String(e)));
      await gotoApp(page, projectPath(suffix));
      // The shell mounted and the lazy chunk resolved.
      await expect(page.locator('#root')).not.toBeEmpty({ timeout: 20_000 });
      await expect(page.getByText('Loading page...', { exact: true })).toHaveCount(0, { timeout: 20_000 });
      await expectNoErrorUI(page);
      expect(consoleErrors, `${suffix} threw during mount`).toEqual([]);
    });
  }

  for (const [name, path] of GLOBAL_ROUTES) {
    test(`global route: ${name}`, async ({ page }) => {
      const consoleErrors: string[] = [];
      page.on('pageerror', (e) => consoleErrors.push(String(e)));
      await gotoApp(page, path);
      await expect(page.locator('#root')).not.toBeEmpty({ timeout: 20_000 });
      await expect(page.getByText('Loading page...', { exact: true })).toHaveCount(0, { timeout: 20_000 });
      await expectNoErrorUI(page);
      expect(consoleErrors, `${path} threw during mount`).toEqual([]);
    });
  }

  // Parameterised routes need a real id, discovered read-only from the API.
  test('run detail renders for the newest run', async ({ page, request }) => {
    const runs = await apiGet<Array<{ id: string }>>(request, `/v2/projects/${process.env.PROD_SMOKE_PROJECT_ID}/test-runs?limit=1`);
    test.skip(runs.length === 0, 'no runs in the smoke project');
    const consoleErrors: string[] = [];
    page.on('pageerror', (e) => consoleErrors.push(String(e)));
    await gotoApp(page, projectPath(`/runs/${runs[0].id}`));
    await expect(page.locator('#root')).not.toBeEmpty({ timeout: 20_000 });
    await expectNoErrorUI(page);
    expect(consoleErrors, 'run detail threw during mount').toEqual([]);
  });
});
