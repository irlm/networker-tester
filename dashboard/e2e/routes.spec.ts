import { test } from '@playwright/test';
import {
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
