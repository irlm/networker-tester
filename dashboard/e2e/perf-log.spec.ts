import { expect, test } from '@playwright/test';
import { expectRouteToRender, seedSession, stubRuntime, watchForFatalErrors } from './support/runtime';

test('performance logs default to a five-minute server query and preserve range in the URL', async ({ page }) => {
  const perfLogRequests: URL[] = [];
  page.on('request', request => {
    const url = new URL(request.url());
    if (url.pathname.endsWith('/api/perf-log')) perfLogRequests.push(url);
  });
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  const beforeNavigation = Date.now();

  await expectRouteToRender(page, '/admin/perf-log', failures);

  const range = page.getByRole('combobox', { name: 'Time range' });
  await expect(range).toHaveValue('5m');
  await expect.poll(() => perfLogRequests.length).toBeGreaterThan(0);
  const since = perfLogRequests.at(-1)?.searchParams.get('since');
  expect(since).not.toBeNull();
  const requestedWindow = Date.now() - new Date(since!).getTime();
  expect(requestedWindow).toBeGreaterThanOrEqual(5 * 60_000);
  expect(requestedWindow).toBeLessThan(5 * 60_000 + (Date.now() - beforeNavigation) + 1_000);

  await range.selectOption('1h');
  await expect(page).toHaveURL(/\?range=1h$/);
  await expect.poll(() => perfLogRequests.at(-1)?.searchParams.get('since')).not.toBe(since);
});
