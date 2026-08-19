import { expect, test } from '@playwright/test';
import {
  GROUP_E2E_ID,
  PID,
  expectRouteToRender,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

// #803: a comparison group renders as ONE expandable row on the runs list,
// links to its compare page, and stays a flat list under ?comparison_group=.
test('comparison group clusters into one row, expands to cells, and opens the compare page', async ({ page }) => {
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}/runs`, failures);

  // One group row (name from the group API), cells hidden until expanded.
  const groupLink = page.getByRole('link', { name: 'Stack shootout' });
  await expect(groupLink).toBeVisible();
  await expect(page.getByText('lab/loop linux · nginx · cg-cge2e·0·ab12')).toHaveCount(0);
  // 2 terminal cells of the group's 3 defined (authoritative count, no "+").
  await expect(page.getByText('2/3')).toBeVisible();
  // Standalone rows are untouched siblings.
  await expect(page.getByText('Checkout connectivity')).toBeVisible();

  // Expand → cell rows with their unchanged run URLs + the list affordance.
  await page.getByRole('button', { name: /Expand group Stack shootout/ }).click();
  await expect(page.getByText('lab/loop linux · nginx · cg-cge2e·0·ab12')).toBeVisible();
  await expect(page.getByRole('link', { name: 'run-cell' }).first()).toHaveAttribute(
    'href', `/projects/${PID}/runs/run-cell-nginx`,
  );
  await expect(page.getByRole('link', { name: 'view as list' })).toHaveAttribute(
    'href', `/projects/${PID}/runs?comparison_group=${GROUP_E2E_ID}`,
  );

  // Group name → compare page, which renders the group header and its cells.
  await groupLink.click();
  await expect(page).toHaveURL(new RegExp(`benchmarks/compare/${GROUP_E2E_ID}`));
  await expect(page.getByRole('heading', { name: 'Stack shootout' })).toBeVisible();

  // "view as list" filter → the flat per-cell list (no group row).
  await page.goto(`/projects/${PID}/runs?comparison_group=${GROUP_E2E_ID}`);
  await expect(page.getByText('lab/loop linux · nginx · cg-cge2e·0·ab12')).toBeVisible();
  await expect(page.getByRole('button', { name: /Expand group/ })).toHaveCount(0);
  expect(failures()).toEqual([]);
});
