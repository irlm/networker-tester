import { test, expect, gotoApp, projectPath, expectNoErrorUI, COMPARE_GROUP } from './support';

// Spec 7 — comparison-group pivots for a known group (#794 surface).
// Optional: needs PROD_SMOKE_COMPARE_GROUP in .prod-smoke.env; skipped with a
// note when unset. READ-ONLY: render assertions only.

test.describe('comparison pivots', () => {
  test('comparison page renders pivots with per-cell run links', async ({ page }) => {
    test.skip(
      COMPARE_GROUP === '',
      'PROD_SMOKE_COMPARE_GROUP not set — add a known comparison-group id to .prod-smoke.env to cover the pivots',
    );

    await gotoApp(page, projectPath(`/benchmarks/compare/${COMPARE_GROUP}`));

    // Both pivots are offered.
    const pivotNav = page.locator('[aria-label="Comparison pivot"]');
    await expect(pivotNav).toBeVisible();
    await expect(pivotNav.getByRole('button', { name: 'By testbed' })).toBeVisible();
    await expect(pivotNav.getByRole('button', { name: 'By language' })).toBeVisible();

    // Per-cell rows link back to their runs ("run <id8> →").
    await expect
      .poll(() => page.getByRole('link', { name: /^run [0-9a-f]{8}/ }).count(), {
        message: 'per-cell run links should render',
      })
      .toBeGreaterThanOrEqual(1);
    const firstRunLink = page.getByRole('link', { name: /^run [0-9a-f]{8}/ }).first();
    expect(await firstRunLink.getAttribute('href')).toMatch(/\/runs\//);

    // The other pivot renders too (client-side toggle, no mutation).
    await pivotNav.getByRole('button', { name: 'By language' }).click();
    await expect(pivotNav.getByRole('button', { name: 'By language' })).toHaveAttribute('aria-pressed', 'true');

    await expectNoErrorUI(page);
  });
});
