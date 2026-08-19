import { test, expect, gotoApp, projectPath, expectNoErrorUI } from './support';

// Spec 3 — URL Probe page render. READ-ONLY: no probe is launched.

test.describe('url probe surface', () => {
  test('url probe page renders input and watchlist', async ({ page }) => {
    await gotoApp(page, projectPath('/probe'));

    await expect(page.getByRole('heading', { name: 'URL Probe' })).toBeVisible();

    // The probe input bar (never submitted by this harness).
    await expect(page.locator('#diag-url')).toBeVisible();
    await expect(page.getByText('Probe a URL')).toBeVisible();

    // Watchlist section: "Watched URLs (N)" toolbar renders regardless of
    // whether the project watches anything yet.
    await expect(page.getByText(/Watched URLs \(\d+\)/)).toBeVisible();

    await expectNoErrorUI(page);
  });
});
