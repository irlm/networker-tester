import { expect, test } from '@playwright/test';
import { PID, expectRouteToRender, seedSession, stubRuntime, watchForFatalErrors } from './support/runtime';

test.describe('start-a-test triage console', () => {
  test.beforeEach(async ({ page }) => {
    const failures = watchForFatalErrors(page);
    await stubRuntime(page);
    await seedSession(page);
    await expectRouteToRender(page, `/projects/${PID}/scenarios`, failures);
  });

  test('supports the primary flow without a pointer', async ({ page }) => {
    await expect(page.getByRole('heading', { name: 'Quick latency & TLS check' })).toBeVisible();

    await page.keyboard.press('3');
    await expect(page.getByRole('tab', { name: /Deployed endpoint/ })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('heading', { name: 'Throughput to your endpoint' })).toBeVisible();

    await page.keyboard.press('m');
    await expect(page.getByText(/Uses the existing tested builder/)).toBeVisible();

    await page.keyboard.press('s');
    await expect(page.getByText(/Refreshes every 15 seconds/)).toBeVisible();
  });

  test('can turn character shortcuts off', async ({ page }) => {
    await page.getByRole('button', { name: /Scenario keyboard shortcuts enabled/ }).click();
    await expect(page.getByRole('button', { name: /Scenario keyboard shortcuts disabled/ })).toHaveAttribute('aria-pressed', 'false');

    await page.keyboard.press('4');
    await expect(page.getByRole('tab', { name: /URL symptoms/ })).toHaveAttribute('aria-selected', 'true');
  });

  test('finds catalog tests through the shared command palette', async ({ page }) => {
    await page.keyboard.press('/');
    const search = page.getByPlaceholder(/Jump to a page or search docs/);
    await search.fill('WebSocket latency');

    const result = page.getByRole('button', { name: /test.*WebSocket latency/i });
    await expect(result).toBeVisible();
    await result.click();

    await expect(page).toHaveURL(new RegExp(`/projects/${PID}/tests/new\\?modes=websocket$`));
  });
});
