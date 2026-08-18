import { expect, test } from '@playwright/test';
import {
  PID,
  expectRouteToRender,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

const routes = [
  ['dashboard', `/projects/${PID}`],
  ['runs', `/projects/${PID}/runs`],
  ['run detail', `/projects/${PID}/runs/run-e2e-1`],
  ['network test', `/projects/${PID}/tests/new`],
  ['URL probe', `/projects/${PID}/probe`],
  ['infrastructure', `/projects/${PID}/vms`],
  ['schedules', `/projects/${PID}/schedules`],
  ['alerts', `/projects/${PID}/alerts`],
  ['value report', `/projects/${PID}/reports/value`],
  ['performance log', '/admin/perf-log'],
] as const;

const viewports = [
  ['mobile', { width: 390, height: 844 }],
  ['tablet', { width: 768, height: 1024 }],
  ['desktop', { width: 1440, height: 900 }],
] as const;

test.describe('representative routes adapt without document overflow', () => {
  for (const [viewportName, viewport] of viewports) {
    for (const [routeName, path] of routes) {
      test(`${routeName} · ${viewportName}`, async ({ page }) => {
        await page.setViewportSize(viewport);
        const failures = watchForFatalErrors(page);
        await stubRuntime(page);
        await seedSession(page);
        await expectRouteToRender(page, path, failures);

        const dimensions = await page.evaluate(() => ({
          viewport: document.documentElement.clientWidth,
          document: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth),
        }));
        expect(
          dimensions.document,
          `${path} is ${dimensions.document - dimensions.viewport}px wider than the ${viewportName} viewport`,
        ).toBeLessThanOrEqual(dimensions.viewport + 1);
      });
    }
  }
});
