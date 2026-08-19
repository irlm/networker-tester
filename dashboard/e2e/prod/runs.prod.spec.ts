import { test, expect, gotoApp, projectPath, expectNoErrorUI, listRecentRuns } from './support';

// Specs 1, 2, 8 — Runs list, purpose-tab filtering, and run detail.
// READ-ONLY: navigation + render assertions; nothing is launched or mutated.

// Mirror of src/lib/ansi.ts stripAnsi (CSI subset): error_message can carry
// tester SGR color codes that the banner strips before rendering.
const ANSI_CSI = new RegExp(String.fromCharCode(0x1b) + '\\[[0-9;:?]*[ -/]*[@-~]', 'g');

test.describe('runs surface', () => {
  test('runs list boots with the table and purpose tabs', async ({ page }) => {
    await gotoApp(page, projectPath('/runs'));

    await expect(page.getByRole('heading', { name: 'Runs', exact: true })).toBeVisible();

    const tabs = page.locator('[aria-label="Run purpose"]');
    await expect(tabs).toBeVisible();
    for (const label of ['All', 'Network tests', 'URL probes', 'SDK probes', 'Benchmarks']) {
      await expect(tabs.getByRole('button', { name: label, exact: true })).toBeVisible();
    }

    // The list renders as a table (skeleton first, then rows or an empty
    // state) — a blank page or the error state must fail.
    await expect(page.locator('table').first()).toBeVisible();
    await expect(page.getByText('Failed to load runs')).toHaveCount(0);
    await expectNoErrorUI(page);
  });

  test('switching a purpose tab updates the list without errors', async ({ page }) => {
    await gotoApp(page, projectPath('/runs'));
    const tabs = page.locator('[aria-label="Run purpose"]');
    await expect(tabs).toBeVisible();

    // Cycle a couple of tabs; the console/pageerror listeners in support.ts
    // fail the spec on any uncaught error this churn produces.
    await tabs.getByRole('button', { name: 'URL probes', exact: true }).click();
    await expect(page).toHaveURL(/test_kind=url_probe/);
    await expect(page.locator('table').first()).toBeVisible();

    await tabs.getByRole('button', { name: 'Benchmarks', exact: true }).click();
    await expect(page).toHaveURL(/test_kind=benchmark/);
    await expect(page.locator('table').first()).toBeVisible();

    await tabs.getByRole('button', { name: 'All', exact: true }).click();
    await expect(page.locator('table').first()).toBeVisible();
    await expectNoErrorUI(page);
  });

  test('run detail of the newest finished run renders header, status and detail', async ({ page, request }) => {
    const runs = await listRecentRuns(request);
    const finished = runs.filter((r) =>
      r.status === 'completed' || r.status === 'failed' || r.status === 'cancelled');
    test.skip(finished.length === 0, 'no finished runs in the project — nothing to render');

    // Newest completed run first; fall back to the newest terminal run so a
    // freshly-wiped project still exercises the page.
    const run = finished.find((r) => r.status === 'completed') ?? finished[0];
    const shortId = run.id.slice(0, 8);

    await gotoApp(page, projectPath(`/runs/${run.id}`));
    await expect(page.getByRole('heading', { name: `Run ${shortId}` })).toBeVisible();
    // Status chip next to the header.
    await expect(page.getByText(run.status, { exact: true }).first()).toBeVisible();
    // Attempts summary or the degraded/error surface — never a blank page.
    await expect(page.getByText(/attempts/).first()).toBeVisible();
    await expectNoErrorUI(page);

    // #795 regression guard: a failed run must render its error_message
    // (data-testid=run-error-banner), not a bare FAILED badge.
    const failedWithMessage = finished.find((r) => r.status === 'failed' && r.error_message);
    if (failedWithMessage) {
      await gotoApp(page, projectPath(`/runs/${failedWithMessage.id}`));
      await expect(page.getByRole('heading', { name: `Run ${failedWithMessage.id.slice(0, 8)}` })).toBeVisible();
      const banner = page.getByTestId('run-error-banner');
      await expect(banner).toBeVisible();
      // The banner strips ANSI (src/lib/ansi.ts) before rendering — compare
      // against a short ANSI-stripped prefix of the API's error_message.
      const cleaned = (failedWithMessage.error_message ?? '').replace(ANSI_CSI, '').trim();
      await expect(banner).toContainText(cleaned.slice(0, 40) || 'error');
      await expectNoErrorUI(page);
    }
  });
});
