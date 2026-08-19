import { test, expect, gotoApp, expectNoErrorUI, detectPlatformAdmin } from './support';

// Spec 9 — canary panel render. READ-ONLY: absolutely no dispatch click.
// Requires a platform-admin token; skipped (with a note) otherwise.

test.describe('admin canary surface', () => {
  test('canary panel renders trigger, dispatch history and recent runs', async ({ page, request }) => {
    const admin = await detectPlatformAdmin(request);
    test.skip(!admin, 'PROD_SMOKE_TOKEN is not a platform-admin token — /admin/canary is not visible to it');

    await gotoApp(page, '/admin/canary');

    await expect(page.getByRole('heading', { name: 'Run-execution canary' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Trigger' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Dispatch history' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Recent runs on GitHub' })).toBeVisible();

    // The dispatch button exists but is NEVER clicked by this harness.
    await expect(page.getByRole('button', { name: /Run canary/ })).toBeVisible();

    await expectNoErrorUI(page);
  });
});
