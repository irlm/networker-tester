import { expect, test, type Page } from '@playwright/test';
import {
  PID,
  expectRouteToRender,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

function watchForChartRuntime(page: Page) {
  const requests: string[] = [];
  page.on('request', request => {
    const path = new URL(request.url()).pathname;
    if (/\/assets\/BarChart-[^/]+\.js$/.test(path)) requests.push(path);
  });
  return requests;
}

test.describe('heavy chart runtime is demand-loaded', () => {
  test('non-chart and empty report routes do not download Recharts', async ({ page }) => {
    const failures = watchForFatalErrors(page);
    const chartRequests = watchForChartRuntime(page);
    await stubRuntime(page);
    await seedSession(page);

    for (const path of [
      `/projects/${PID}`,
      `/projects/${PID}/runs/run-e2e-1`,
      `/projects/${PID}/reports/value`,
    ]) {
      await expectRouteToRender(page, path, failures);
    }

    expect(chartRequests).toEqual([]);
  });

  test('a populated distribution requests the chart runtime', async ({ page }) => {
    const failures = watchForFatalErrors(page);
    const chartRequests = watchForChartRuntime(page);
    await stubRuntime(page);
    await page.route('**/api/v2/test-runs/run-e2e-1/attempts', route => route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        attempts: [10, 20, 30].map((ttfb, index) => ({
          attempt_id: `attempt-e2e-${index}`,
          run_id: 'run-e2e-1',
          protocol: 'http2',
          sequence_num: index + 1,
          started_at: new Date(0).toISOString(),
          finished_at: new Date(1_000).toISOString(),
          success: true,
          retry_count: 0,
          http: {
            status_code: 200,
            ttfb_ms: ttfb,
            total_duration_ms: ttfb + 5,
            negotiated_version: 'HTTP/2',
          },
        })),
      }),
    }));
    await seedSession(page);

    await expectRouteToRender(page, `/projects/${PID}/runs/run-e2e-1`, failures);
    await expect(page.getByRole('heading', { name: 'TTFB distribution (ms)' })).toBeVisible();
    await expect.poll(() => chartRequests.length).toBeGreaterThan(0);
  });
});
