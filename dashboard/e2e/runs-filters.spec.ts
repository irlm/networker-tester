import { expect, test } from '@playwright/test';
import {
  PID,
  expectRouteToRender,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

test('run purpose tabs and secondary filters compose in the production bundle', async ({ page }) => {
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}/runs`, failures);

  await expect(page.getByRole('button', { name: 'URL probes' })).toBeVisible();
  await page.getByRole('button', { name: 'URL probes' }).click();
  await expect(page.getByText('Diag: api.example.com (Quick)')).toBeVisible();
  await expect(page.getByText('Checkout connectivity')).toHaveCount(0);

  await page.getByRole('button', { name: 'All' }).click();
  await expect(page).not.toHaveURL(/test_kind=/);
  await page.getByLabel('Filter by target type').selectOption('runtime');
  await page.getByLabel('Filter by mode family').selectOption('thru');
  await expect(page.getByText('Runtime throughput')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Benchmarks' })).toBeVisible();

  await page.getByLabel('Search runs by name').fill('Runtime');
  await expect(page).toHaveURL(/q=Runtime/);
  expect(failures()).toEqual([]);
});
