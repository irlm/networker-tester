import { test, expect, gotoApp, projectPath, requireProjectId, expectNoErrorUI } from './support';

// Spec 4 — "Start a test" scenario launcher. READ-ONLY: the only click is a
// navigation into a configure page; nothing is launched.
//
// The page is a TABBED triage console (#799): a "Test intent" tablist, and only
// the selected tab's scenarios are in the DOM. The original assertions were
// written against the flat card grid that preceded it and had drifted twice
// over — they demanded >= 4 action links on one view (the URL-symptoms tab
// legitimately offers 3) and matched the action link by the literal text
// "Configure", when the label is `availability.actionLabel` and changes with
// readiness ("View status" on a project that cannot configure yet). Both now
// key off the `data-scenario-action` hook the component provides for exactly
// this, and walk the tabs instead of assuming one flat list.

const ACTION = '[data-scenario-action]';

test.describe('start-a-test surface', () => {
  test('every intent tab offers scenarios whose actions are project-scoped', async ({ page }) => {
    await gotoApp(page, projectPath('/scenarios'));

    await expect(page.getByRole('heading', { name: 'Start a test' })).toBeVisible();

    const tabs = page.getByRole('tablist', { name: 'Test intent' }).getByRole('tab');
    const tabCount = await tabs.count();
    expect(tabCount, 'intent tabs should render').toBeGreaterThanOrEqual(4);

    let total = 0;
    for (let t = 0; t < tabCount; t++) {
      const label = (await tabs.nth(t).textContent())?.trim() ?? `tab ${t}`;
      await tabs.nth(t).click();

      // Each tab shows a recommended scenario plus any alternatives; every one
      // of them must carry a real, project-scoped href (a broken action would
      // render as "#" or empty).
      const actions = page.locator(ACTION);
      await expect(actions.first(), `${label} should offer at least one scenario`).toBeVisible();

      const n = await actions.count();
      total += n;
      for (let i = 0; i < n; i++) {
        const href = await actions.nth(i).getAttribute('href');
        expect(href, `${label} action ${i} href`).toBeTruthy();
        expect(href, `${label} action ${i} must stay inside the project`)
          .toContain(`/projects/${requireProjectId()}/`);
      }
      await expectNoErrorUI(page);
    }

    // Across all intents the console must still offer a real menu, not one card.
    expect(total, 'scenarios across all intent tabs').toBeGreaterThanOrEqual(4);
  });

  test('a url-probe scenario routes to its configure page, or to runners when not ready', async ({ page }) => {
    await gotoApp(page, projectPath('/scenarios'));
    await expect(page.getByRole('heading', { name: 'Start a test' })).toBeVisible();

    const tabs = page.getByRole('tablist', { name: 'Test intent' }).getByRole('tab');
    await expect(tabs.first()).toBeVisible();
    await tabs.first().click();

    // The console is READINESS-AWARE (#799): with an online runner a
    // URL-symptom scenario configures on /probe; with none it offers
    // "View runners →" to /vms instead, because there is nothing to run on.
    // Both are correct — asserting only the first made this spec fail whenever
    // the smoke project had no runner up, which is its normal resting state.
    const actions = page.locator(ACTION);
    await expect(actions.first()).toBeVisible();

    const probeAction = page.locator(`${ACTION}[href*="/probe"]`);
    if (await probeAction.count() > 0) {
      await probeAction.first().click();
      await expect(page).toHaveURL(/\/probe(\?|$)/);
      await expect(page.getByRole('heading', { name: 'URL Probe' })).toBeVisible();
      await expect(page.locator('#diag-url')).toBeVisible();
    } else {
      // Not ready: every offered action must send the user somewhere that can
      // FIX that — the runners page — rather than a dead end.
      const hrefs = await actions.evaluateAll((els) =>
        els.map((e) => (e as HTMLAnchorElement).getAttribute('href') ?? ''));
      expect(hrefs.length, 'the tab must still offer an action').toBeGreaterThan(0);
      for (const href of hrefs) {
        expect(href, 'a readiness-blocked scenario must route to the runners page')
          .toContain('/vms');
      }
      await actions.first().click();
      await expect(page).toHaveURL(/\/vms$/);
      await expect(page.getByRole('heading', { name: /Infrastructure|Runners/ }).first()).toBeVisible();
    }

    await expectNoErrorUI(page);
  });
});
