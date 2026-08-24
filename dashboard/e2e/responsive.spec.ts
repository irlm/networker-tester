import { devices, expect, test, type Page } from '@playwright/test';
import {
  PID,
  authenticatedRoutes,
  expectRouteToRender,
  publicRoutes,
  seedSession,
  stubRuntime,
  watchForFatalErrors,
} from './support/runtime';

const routeViewports = [
  ['small mobile', { width: 320, height: 568 }],
  ['tablet portrait', { width: 820, height: 1180 }],
  ['desktop', { width: 1440, height: 900 }],
] as const;

const widthMatrix = [
  [320, 568],
  [360, 640],
  [375, 667],
  [390, 844],
  [412, 915],
  [430, 932],
  [480, 800],
  [600, 960],
  [768, 1024],
  [820, 1180],
  [1024, 768],
  [1180, 820],
  [1280, 800],
  [1366, 768],
  [1440, 900],
  [1920, 1080],
] as const;

async function expectNoDocumentOverflow(page: Page, label: string) {
  const dimensions = await page.evaluate(() => ({
    viewport: document.documentElement.clientWidth,
    document: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth),
  }));
  expect(
    dimensions.document,
    `${label} is ${dimensions.document - dimensions.viewport}px wider than its viewport`,
  ).toBeLessThanOrEqual(dimensions.viewport + 1);
}

test.describe('every routed page fits representative viewport classes', () => {
  for (const [viewportName, viewport] of routeViewports) {
    for (const [routeName, path] of authenticatedRoutes) {
      test(`${routeName} · ${viewportName}`, async ({ page }) => {
        await page.setViewportSize(viewport);
        const failures = watchForFatalErrors(page);
        await stubRuntime(page);
        await seedSession(page);
        await expectRouteToRender(page, path, failures);
        await expectNoDocumentOverflow(page, `${path} at ${viewportName}`);
      });
    }

    for (const [routeName, path] of publicRoutes) {
      test(`${routeName} · ${viewportName}`, async ({ page }) => {
        await page.setViewportSize(viewport);
        const failures = watchForFatalErrors(page);
        await stubRuntime(page);
        await expectRouteToRender(page, path, failures);
        await expectNoDocumentOverflow(page, `${path} at ${viewportName}`);
      });
    }
  }
});

test.describe('the full width matrix preserves the data-heavy runs layout', () => {
  for (const [width, height] of widthMatrix) {
    test(`${width} × ${height}`, async ({ page }) => {
      await page.setViewportSize({ width, height });
      const failures = watchForFatalErrors(page);
      await stubRuntime(page);
      await seedSession(page);
      await expectRouteToRender(page, `/projects/${PID}/runs`, failures);
      await expectNoDocumentOverflow(page, `runs at ${width} × ${height}`);
      await expect(page.getByRole('heading', { name: 'Runs' })).toBeVisible();
    });
  }
});

test('continuous resizing stays stable through and between breakpoints', async ({ page }) => {
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await page.setViewportSize({ width: 320, height: 800 });
  await expectRouteToRender(page, `/projects/${PID}/runs`, failures);

  for (let width = 320; width <= 1500; width += 17) {
    await page.setViewportSize({ width, height: 800 });
    await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())));
    await expectNoDocumentOverflow(page, `runs at continuously resized width ${width}`);
  }
});

test('touch mobile navigation remains discoverable and operable', async ({ browser }) => {
  const context = await browser.newContext({
    ...devices['iPhone 13'],
    viewport: { width: 390, height: 844 },
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}`, failures);

  const toggle = page.getByRole('button', { name: 'Toggle navigation' });
  await expect(toggle).toBeVisible();
  await toggle.tap();
  const navigation = page.getByRole('navigation', { name: 'Main navigation' });
  await expect(navigation).toBeVisible();
  await page.getByRole('link', { name: 'Runs', exact: true }).tap();
  await expect(page).toHaveURL(new RegExp(`/projects/${PID}/runs$`));
  await expect(navigation).toBeHidden();
  await expectNoDocumentOverflow(page, 'touch mobile navigation destination');
  await context.close();
});

test('small-phone navigation is contained by the viewport and all footer actions remain reachable', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 568 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}`, failures);

  await page.getByRole('button', { name: 'Toggle navigation' }).tap();
  const sidebar = page.getByRole('complementary');
  const navigation = page.getByRole('navigation', { name: 'Main navigation' });
  const sidebarBounds = await sidebar.boundingBox();
  expect(sidebarBounds).not.toBeNull();
  expect(sidebarBounds!.y).toBeGreaterThanOrEqual(0);
  expect(sidebarBounds!.y + sidebarBounds!.height).toBeLessThanOrEqual(568);

  const settingsLink = navigation.getByRole('link', { name: 'Settings' });
  await navigation.evaluate(element => element.scrollTo({ top: element.scrollHeight }));
  await expect(settingsLink).toBeVisible();
  expect(await navigation.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  await expect(page.getByRole('button', { name: 'Change password' })).toBeVisible();
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  await context.close();
});

test('mobile drawer ignores the persisted desktop collapsed preference', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 568 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await page.addInitScript(() => localStorage.setItem('sidebar-collapsed', '1'));
  await expectRouteToRender(page, `/projects/${PID}`, failures);

  const toggle = page.getByRole('button', { name: 'Toggle navigation' });
  await toggle.tap();
  const sidebar = page.getByRole('complementary');
  const projectName = page.getByText('E2E Project', { exact: true });
  await expect(sidebar.getByRole('link', { name: 'Runs', exact: true })).toBeVisible();
  const [sidebarBounds, toggleBounds, projectBounds] = await Promise.all([
    sidebar.boundingBox(),
    toggle.boundingBox(),
    projectName.boundingBox(),
  ]);
  expect(sidebarBounds).not.toBeNull();
  expect(toggleBounds).not.toBeNull();
  expect(projectBounds).not.toBeNull();
  expect(sidebarBounds!.width).toBeGreaterThanOrEqual(190);
  expect(projectBounds!.y).toBeGreaterThanOrEqual(toggleBounds!.y + toggleBounds!.height);
  await context.close();
});

test('mobile run-purpose tabs expose every option through their own scroller', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 568 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}/runs`, failures);

  const tabs = page.getByLabel('Run purpose');
  const benchmarks = tabs.getByRole('button', { name: 'Benchmarks' });
  await benchmarks.scrollIntoViewIfNeeded();
  await benchmarks.tap();
  await expect(benchmarks).toHaveClass(/border-cyan-500/);
  expect(await tabs.evaluate(element => element.scrollLeft)).toBeGreaterThan(0);
  await expectNoDocumentOverflow(page, 'mobile run-purpose tabs');
  await context.close();
});

test('mobile slide-over keeps its close control and final actions reachable', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 568 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}/sdk-endpoints`, failures);

  await page.getByRole('button', { name: '+ SDK endpoint' }).tap();
  const dialog = page.getByRole('dialog', { name: 'Register SDK endpoint' });
  await expect(dialog).toBeVisible();
  await dialog.evaluate(element => Promise.all(element.getAnimations().map(animation => animation.finished)));
  const bounds = await dialog.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.x).toBeGreaterThanOrEqual(-1);
  expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(321);
  expect(bounds!.y).toBeGreaterThanOrEqual(0);
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(568);
  expect(await dialog.evaluate(element => getComputedStyle(element).backgroundColor)).toBe('rgb(10, 11, 15)');

  const register = dialog.getByRole('button', { name: 'Register endpoint' });
  await register.scrollIntoViewIfNeeded();
  await expect(register).toBeVisible();
  await dialog.getByRole('button', { name: 'Close' }).click();
  await expect(dialog).toBeHidden();
  await context.close();
});

test('mobile performance log trigger stays compact and its expanded tools wrap inside the viewport', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 568 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}/runs`, failures);

  const trigger = page.getByRole('button', { name: 'Open performance log' });
  const triggerBounds = await trigger.boundingBox();
  expect(triggerBounds).not.toBeNull();
  expect(triggerBounds!.width).toBeLessThanOrEqual(52);
  expect(triggerBounds!.height).toBeGreaterThanOrEqual(44);
  await trigger.tap();

  const panel = page.getByRole('region', { name: 'Performance log' });
  await expect(panel).toBeVisible();
  const panelBounds = await panel.boundingBox();
  expect(panelBounds).not.toBeNull();
  expect(panelBounds!.x).toBeGreaterThanOrEqual(-1);
  expect(panelBounds!.x + panelBounds!.width).toBeLessThanOrEqual(321);
  await expect(panel.getByRole('button', { name: 'Close performance log' })).toBeVisible();
  await expect(panel.getByPlaceholder('Filter by path...')).toBeVisible();
  await panel.getByRole('button', { name: 'Close performance log' }).tap();
  await expect(panel).toBeHidden();
  await context.close();
});

test('help and command overlays fit a small phone viewport', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 568 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}`, failures);

  await page.getByRole('button', { name: 'Toggle navigation' }).tap();
  await page.getByRole('button', { name: '? Help' }).tap();
  const help = page.getByRole('dialog', { name: 'Help' });
  await expect(help).toBeVisible();
  const helpBounds = await help.boundingBox();
  expect(helpBounds).not.toBeNull();
  expect(helpBounds!.x).toBeGreaterThanOrEqual(-1);
  expect(helpBounds!.x + helpBounds!.width).toBeLessThanOrEqual(321);
  expect(helpBounds!.y).toBeGreaterThanOrEqual(0);
  expect(helpBounds!.y + helpBounds!.height).toBeLessThanOrEqual(568);
  expect(await help.evaluate(element => element.scrollWidth)).toBeLessThanOrEqual(
    await help.evaluate(element => element.clientWidth + 1),
  );
  await expect(help.getByRole('button', { name: /TCP Connect/ })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(help).toBeHidden();

  await page.getByRole('button', { name: 'Toggle navigation' }).tap();
  await page.getByRole('button', { name: '/ Search' }).tap();
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await expect(palette).toBeVisible();
  const paletteBounds = await palette.boundingBox();
  expect(paletteBounds).not.toBeNull();
  expect(paletteBounds!.x).toBeGreaterThanOrEqual(-1);
  expect(paletteBounds!.x + paletteBounds!.width).toBeLessThanOrEqual(321);
  expect(paletteBounds!.y + paletteBounds!.height).toBeLessThanOrEqual(568);
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  await expect(palette).toBeHidden();
  await context.close();
});

test('centered dialog remains usable in phone landscape', async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 844, height: 390 },
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 3,
  });
  const page = await context.newPage();
  const failures = watchForFatalErrors(page);
  await stubRuntime(page);
  await seedSession(page);
  await expectRouteToRender(page, `/projects/${PID}`, failures);

  const toggle = page.getByRole('button', { name: 'Toggle navigation' });
  await expect(toggle).toBeVisible();
  await toggle.tap();
  await page.getByRole('button', { name: 'Change password' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  const bounds = await dialog.boundingBox();
  expect(bounds).not.toBeNull();
  expect(bounds!.y).toBeGreaterThanOrEqual(0);
  expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(390);
  await expect(dialog.getByRole('button', { name: /update password/i })).toBeVisible();
  await dialog.getByRole('button', { name: /cancel/i }).click();
  await expect(dialog).toBeHidden();
  await context.close();
});

test.describe('responsive audit screenshots', () => {
  test.skip(process.env.RESPONSIVE_SCREENSHOTS !== '1', 'opt-in local audit evidence');

  for (const [name, viewport, path] of [
    ['mobile-dashboard', { width: 320, height: 568 }, `/projects/${PID}`],
    ['mobile-runs', { width: 390, height: 844 }, `/projects/${PID}/runs`],
    ['tablet-infrastructure', { width: 820, height: 1180 }, `/projects/${PID}/vms`],
    ['laptop-network-test', { width: 1280, height: 800 }, `/projects/${PID}/tests/new`],
    ['desktop-run-detail', { width: 1920, height: 1080 }, `/projects/${PID}/runs/run-e2e-1`],
  ] as const) {
    test(name, async ({ browser }, testInfo) => {
      const context = await browser.newContext({
        viewport,
        ...(name.startsWith('mobile') ? { hasTouch: true, isMobile: true, deviceScaleFactor: 1 } : {}),
      });
      const page = await context.newPage();
      const failures = watchForFatalErrors(page);
      await stubRuntime(page);
      await seedSession(page);
      await expectRouteToRender(page, path, failures);
      await page.screenshot({ path: testInfo.outputPath(`${name}.png`), fullPage: true });
      await context.close();
    });
  }

  test('mobile-navigation-state', async ({ browser }, testInfo) => {
    const context = await browser.newContext({
      viewport: { width: 320, height: 568 },
      hasTouch: true,
      isMobile: true,
      deviceScaleFactor: 1,
    });
    const page = await context.newPage();
    await stubRuntime(page);
    await seedSession(page);
    const failures = watchForFatalErrors(page);
    await expectRouteToRender(page, `/projects/${PID}`, failures);
    await page.getByRole('button', { name: 'Toggle navigation' }).tap();
    await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('mobile-navigation-state.png') });
    await context.close();
  });

  test('mobile-slide-over-state', async ({ browser }, testInfo) => {
    const context = await browser.newContext({
      viewport: { width: 320, height: 568 },
      hasTouch: true,
      isMobile: true,
      deviceScaleFactor: 1,
    });
    const page = await context.newPage();
    await stubRuntime(page);
    await seedSession(page);
    const failures = watchForFatalErrors(page);
    await expectRouteToRender(page, `/projects/${PID}/sdk-endpoints`, failures);
    await page.getByRole('button', { name: '+ SDK endpoint' }).tap();
    const dialog = page.getByRole('dialog', { name: 'Register SDK endpoint' });
    await dialog.evaluate(element => Promise.all(element.getAnimations().map(animation => animation.finished)));
    await page.screenshot({ path: testInfo.outputPath('mobile-slide-over-state.png') });
    await context.close();
  });

  test('mobile-help-state', async ({ browser }, testInfo) => {
    const context = await browser.newContext({
      viewport: { width: 320, height: 568 },
      hasTouch: true,
      isMobile: true,
      deviceScaleFactor: 1,
    });
    const page = await context.newPage();
    await stubRuntime(page);
    await seedSession(page);
    const failures = watchForFatalErrors(page);
    await expectRouteToRender(page, `/projects/${PID}`, failures);
    await page.getByRole('button', { name: 'Toggle navigation' }).tap();
    await page.getByRole('button', { name: '? Help' }).tap();
    await expect(page.getByRole('dialog', { name: 'Help' })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('mobile-help-state.png') });
    await context.close();
  });
});

/**
 * The desktop perf-log panel must never slide under the sidebar.
 *
 * It is `fixed right-0` at a fixed width, so capping it against the VIEWPORT is
 * not enough: the sidebar (`w-48`, 192 px) occupies the left edge and paints
 * over it, and at ~1080 px a 900 px panel lost exactly the first character of
 * "API", "Filter" and "Time" — visible only in a real render, which is why this
 * is a geometry assertion and not a snapshot.
 */
const SIDEBAR_PX = 192;

for (const width of [1024, 1080, 1280, 1440, 1920]) {
  test(`perf-log panel clears the sidebar at ${width}px`, async ({ page }) => {
    const failures = watchForFatalErrors(page);
    await page.setViewportSize({ width, height: 860 });
    await stubRuntime(page);
    await seedSession(page);
    await expectRouteToRender(page, `/projects/${PID}/runs`, failures);

    await page.getByRole('button', { name: 'Open performance log' }).click();
    const panel = page.getByRole('region', { name: /performance log/i }).first();
    const box = await panel.boundingBox();
    expect(box, 'the panel should be on screen').not.toBeNull();
    expect(
      box!.x,
      `panel starts at ${box!.x}px and would be clipped by the ${SIDEBAR_PX}px sidebar`,
    ).toBeGreaterThanOrEqual(SIDEBAR_PX);
    expect(box!.x + box!.width).toBeLessThanOrEqual(width + 1);
  });
}
