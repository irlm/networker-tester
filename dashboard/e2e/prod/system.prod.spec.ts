import { test, expect, gotoApp, projectPath, expectNoErrorUI } from './support';

// Spec 10 — the Settings page's system panels: versions + deployed targets.
// READ-ONLY: no update button is clicked.

test.describe('system surface', () => {
  test('settings page renders versions panel and deployed targets list', async ({ page }) => {
    await gotoApp(page, projectPath('/settings'));

    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();

    // Versions panel: heading plus the dashboard version line.
    await expect(page.getByRole('heading', { name: 'system versions' })).toBeVisible();
    await expect(page.getByText(/^v\d+\.\d+\.\d+/).first()).toBeVisible();

    // Deployed targets: heading plus either target rows or the honest empty
    // state — a panel that never resolves is a failure.
    await expect(page.getByRole('heading', { name: 'deployed targets' })).toBeVisible();
    const targetsEmpty = page.getByText('No targets deployed yet', { exact: false });
    const anyContent = await Promise.race([
      targetsEmpty.waitFor({ state: 'visible', timeout: 15_000 }).then(() => 'empty' as const).catch(() => null),
      page
        .locator('.section-divider', { hasText: 'deployed targets' })
        .locator('.text-gray-200')
        .first()
        .waitFor({ state: 'visible', timeout: 15_000 })
        .then(() => 'rows' as const)
        .catch(() => null),
    ]);
    expect(anyContent, 'deployed targets should render rows or the empty state').not.toBeNull();

    await expectNoErrorUI(page);
  });
});
