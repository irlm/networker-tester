import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import {
  CMP_A,
  CMP_B,
  PID,
  expectRouteToRender,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

const authenticatedRoutes = [
  ['dashboard', `/projects/${PID}`],
  ['runs', `/projects/${PID}/runs`],
  ['run detail', `/projects/${PID}/runs/run-e2e-1`],
  ['start a test', `/projects/${PID}/scenarios`],
  ['network test', `/projects/${PID}/tests/new`],
  ['URL probe', `/projects/${PID}/probe`],
  ['schedules', `/projects/${PID}/schedules`],
  ['alerts', `/projects/${PID}/alerts`],
  ['value report', `/projects/${PID}/reports/value`],
  // #782 P3 — a checkbox-per-URL picker plus a data-dense scoreboard.
  // #851 shipped checkboxes that failed target-size because axe measures the
  // INPUT's own box and padding on the wrapping <label> does not count, so
  // this route has to be under axe, not merely under the render check.
  ['URL comparison', `/projects/${PID}/probe/compare?urls=${encodeURIComponent(`${CMP_A},${CMP_B}`)}`],
  ['performance log', '/admin/perf-log'],
] as const;

test.describe('representative routes meet automated WCAG AA checks', () => {
  for (const [name, path] of authenticatedRoutes) {
    test(name, async ({ page }) => {
      const failures = watchForFatalErrors(page);
      await stubRuntime(page);
      await seedSession(page);
      await expectRouteToRender(page, path, failures);

      const results = await new AxeBuilder({ page })
        .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'])
        .analyze();

      expect(results.violations.map(violation => ({
        id: violation.id,
        impact: violation.impact,
        nodes: violation.nodes.map(node => ({ target: node.target, html: node.html })),
      }))).toEqual([]);
    });
  }
});
