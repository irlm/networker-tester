import { test, expect, gotoApp, projectPath, expectNoErrorUI } from './support';

// Specs 5, 6 — benchmark wizard first steps. READ-ONLY: adding a testbed row
// and opening the account combobox are purely client-side wizard state; the
// suite never advances to Review and never launches.

test.describe('benchmark wizards', () => {
  test('full stack wizard step 1 loads the cloud-account list', async ({ page }) => {
    await gotoApp(page, projectPath('/benchmarks/full-stack/new'));

    // Step 0 = Testbeds. A bare visit starts with no testbed rows.
    await expect(page.getByText('No testbeds configured', { exact: false })).toBeVisible();

    // Add one testbed (client-side state only) so the cloud-account control
    // renders, then open the combobox and assert the account list loaded.
    await page.getByRole('button', { name: '+ Azure / Linux' }).click();
    const combobox = page.getByRole('combobox').first();
    await expect(combobox).toBeVisible();
    await combobox.focus();

    const listbox = page.locator('#cloud-account-listbox');
    await expect(listbox).toBeVisible();
    // Options render — the project must expose at least one cloud account.
    // No selection is made.
    await expect
      .poll(async () => listbox.locator('[role="option"]').count(), {
        message: 'cloud accounts should load into the combobox',
      })
      .toBeGreaterThanOrEqual(1);

    await expectNoErrorUI(page);
  });

  test('application wizard testbeds step gates on cloud account via ?template=api-compute', async ({ page }) => {
    await gotoApp(page, projectPath('/benchmarks/application/new?template=api-compute'));

    // The template prefill advances to step 2 (Testbeds) with an
    // account-less testbed, so Next is gated with the inline hint.
    await expect(page.getByText('select a cloud account to continue')).toBeVisible();
    // The prefilled testbed row's cloud-account control is present.
    await expect(page.getByRole('combobox').first()).toBeVisible();

    await expectNoErrorUI(page);
  });
});
