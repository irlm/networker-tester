import { test, expect, gotoApp, projectPath, requireProjectId, expectNoErrorUI } from './support';

// Spec 4 — "Start a test" scenario launcher. READ-ONLY: the only click is a
// navigation into a configure page; nothing is launched.

test.describe('start-a-test surface', () => {
  test('scenario cards render and every configure link navigates somewhere', async ({ page }) => {
    await gotoApp(page, projectPath('/scenarios'));

    await expect(page.getByRole('heading', { name: 'Start a test' })).toBeVisible();

    // Every card is a Link ending in "Configure →" — each must carry a
    // project-scoped href (a broken href would render as "#" or empty).
    const cards = page.getByRole('link').filter({ hasText: 'Configure' });
    const count = await cards.count();
    expect(count, 'scenario cards should render').toBeGreaterThanOrEqual(4);
    for (let i = 0; i < count; i++) {
      const href = await cards.nth(i).getAttribute('href');
      expect(href, `card ${i} href`).toBeTruthy();
      expect(href).toContain(`/projects/${requireProjectId()}/`);
    }
    await expectNoErrorUI(page);
  });

  test('a url-probe card navigates end-to-end to its configure page', async ({ page }) => {
    await gotoApp(page, projectPath('/scenarios'));

    // "Quick latency & TLS check" → /probe?preset=quick (lib/scenarios.ts).
    await page.getByRole('link', { name: /Quick latency & TLS check/ }).click();
    await expect(page).toHaveURL(/\/probe\?preset=quick/);
    await expect(page.getByRole('heading', { name: 'URL Probe' })).toBeVisible();
    await expect(page.locator('#diag-url')).toBeVisible();

    await expectNoErrorUI(page);
  });
});
