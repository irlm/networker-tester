import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { ReviewStep } from './ReviewStep';
import { makeTestbed, methodologyForPreset } from './testbed-constants';
import type { CloudAccountSummary, Methodology } from '../../api/types';

// #793 P2-4: the Review step never showed WHICH cloud account a testbed would
// provision against — the decision that caused #791 was invisible on the only
// step autoprovision users see. It now shows name + status per testbed (red
// when unhealthy) and blocks Launch with the reason.

function account(over: Partial<CloudAccountSummary> = {}): CloudAccountSummary {
  return {
    account_id: 'acct-1',
    name: 'AWS prod',
    provider: 'aws',
    region_default: null,
    personal: false,
    status: 'active',
    last_validated: null,
    validation_error: null,
    ...over,
  };
}

function renderStep(over: Partial<Parameters<typeof ReviewStep>[0]> = {}) {
  const tb = makeTestbed(0, 'AWS', 'linux', ['nginx']);
  tb.cloudAccountId = 'acct-1';
  const onSubmit = vi.fn();
  render(
    <ReviewStep
      configName=""
      onConfigNameChange={() => {}}
      namePlaceholder="bench"
      summaryLine="1 testbed"
      testbeds={[tb]}
      methodology={methodologyForPreset('standard') as Methodology}
      workloadLine="10 runs"
      addSchedule={false}
      onAddScheduleChange={() => {}}
      cronExpr="0 0 * * * *"
      onCronExprChange={() => {}}
      isMatrixRun={false}
      submitting={false}
      onSubmit={onSubmit}
      launchLabel="Launch Now"
      {...over}
    />,
  );
  return { onSubmit };
}

describe('ReviewStep — cloud-account visibility + launch gate', () => {
  it('shows the account name on the testbed line (muted when active)', () => {
    renderStep({ cloudAccounts: [account()] });
    const acct = screen.getByTestId('review-account');
    expect(acct).toHaveTextContent('AWS prod');
    expect(acct.className).toContain('text-gray-400');
    expect(screen.queryByTestId('launch-blocked-reason')).not.toBeInTheDocument();
  });

  it('shows a red name + status for an unhealthy account and disables Launch', () => {
    const reason = "cloud account 'AWS prod' is in error state: Invalid access key ID";
    const { onSubmit } = renderStep({
      cloudAccounts: [account({ status: 'error', validation_error: 'Invalid access key ID' })],
      launchBlockedReason: reason,
    });

    const acct = screen.getByTestId('review-account');
    expect(acct).toHaveTextContent('AWS prod (error)');
    expect(acct.className).toContain('text-red-400');

    expect(screen.getByTestId('launch-blocked-reason')).toHaveTextContent(reason);
    const launch = screen.getByRole('button', { name: 'Launch Now' });
    expect(launch).toBeDisabled();
    fireEvent.click(launch);
    expect(onSubmit).not.toHaveBeenCalled();
    // Saving the config stays allowed — only launching is gated.
    expect(screen.getByRole('button', { name: 'Save Config' })).toBeEnabled();
  });

  it('renders no account span when accounts are not provided (other wizards)', () => {
    renderStep();
    expect(screen.queryByTestId('review-account')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Launch Now' })).toBeEnabled();
  });
});
