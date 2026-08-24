import { expect, test } from '@playwright/test';
import {
  PID,
  authenticatedRoutes,
  expectRouteToRender,
  publicRoutes,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

test.describe('every routed page renders', () => {
  for (const [name, path] of authenticatedRoutes) {
    test(name, async ({ page }) => {
      const failures = watchForFatalErrors(page);
      await stubRuntime(page);
      await seedSession(page);
      await expectRouteToRender(page, path, failures);
    });
  }

  for (const [name, path] of publicRoutes) {
    test(name, async ({ page }) => {
      const failures = watchForFatalErrors(page);
      await stubRuntime(page);
      await expectRouteToRender(page, path, failures);
    });
  }
});

/**
 * The SDK Endpoints page against a realistic samples payload.
 *
 * The generic "renders" check above cannot catch a wrong-SHAPED stub: when
 * `samples` is empty (or its rows carry the wrong field names) the page still
 * renders happily — it just renders nothing, or rows with blank labels and
 * blank versions. #848's stub mixed the catalog entry shape (`id`,
 * `sdk_version`) into the per-language STATUS rows, which really are
 * `language` + `label` + `current_version`, and nothing failed. These
 * assertions read the values the panel puts on screen, so the shapes have to
 * agree with SampleView.ToWire() for the test to pass.
 */
test('the SDK Endpoints page renders its samples panel from the server shape', async ({ page }) => {
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}/sdk-endpoints`, failures);

  const panel = page.getByRole('region', { name: 'SDK samples' });
  await expect(panel).toBeVisible();

  // `label` — blank for every row if the status rows carry catalog fields.
  for (const label of ['C#', 'JavaScript', 'Python', 'Rust', 'Go']) {
    await expect(panel.getByRole('heading', { name: label, exact: true })).toBeVisible();
  }

  // `current_version` via versionLine(), and the `state` split that drives it.
  await expect(panel.getByText('1 of 5 deployed', { exact: true })).toBeVisible();
  await expect(panel.getByText(/SDK\s*1\.0\.0\s*·\s*:8105/)).toBeVisible();
  await expect(panel.getByText('http://sample.e2e.invalid:8103/laghound')).toBeVisible();

  // `recommended_action` drives the one action per row; it is undefined for
  // every row when the shape is wrong, and then no row renders an action at
  // all. exact — the panel header's own "Deploy samples" button is not a row
  // action. The deployed row is `reuse` AND already registered, which the
  // panel deliberately renders as "in use" rather than a no-op button.
  await expect(panel.getByRole('button', { name: 'Deploy', exact: true })).toHaveCount(4);
  await expect(panel.getByText('in use', { exact: true })).toBeVisible();
  await expect(panel.getByText('registered', { exact: true })).toBeVisible();
});
