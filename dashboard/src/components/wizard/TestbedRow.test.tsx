import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { MemoryRouter } from 'react-router';
import type { CloudAccountSummary } from '../../api/types';
import { TestbedRow } from './TestbedRow';
import { makeTestbed } from './testbed-constants';

// jsdom has no scrollIntoView; the account combobox calls it on option focus.
Element.prototype.scrollIntoView = Element.prototype.scrollIntoView ?? (() => {});

const accounts: CloudAccountSummary[] = [
  {
    account_id: 'acct-1',
    name: 'azure-prod',
    provider: 'azure',
    region_default: 'eastus',
    personal: false,
    status: 'active',
    last_validated: null,
    validation_error: null,
  } as CloudAccountSummary,
];

function renderRow(over: Partial<Parameters<typeof TestbedRow>[0]> = {}) {
  return render(
    <MemoryRouter>
      <TestbedRow
        testbed={makeTestbed(0, 'Azure', 'linux', ['nginx'])}
        index={0}
        projectId="proj-1"
        cloudAccounts={accounts}
        onUpdate={() => {}}
        onRemove={() => {}}
        {...over}
      />
    </MemoryRouter>,
  );
}

describe('TestbedRow — "Use existing VM" visibility (#793 P2-3)', () => {
  it('shows the control by default (the Deploy wizards honor its value)', () => {
    renderRow();
    expect(screen.getByText(/use existing vm/i)).toBeInTheDocument();
  });

  it('hides the control when the host would silently discard the value', () => {
    renderRow({ hideExistingVm: true });
    expect(screen.queryByText(/use existing vm/i)).not.toBeInTheDocument();
  });
});

describe('TestbedRow — cloud-account selector affordance (#791 item 3)', () => {
  it('renders a chevron on the collapsed combobox so it reads as a dropdown', () => {
    renderRow();
    const chevron = screen.getByTestId('cloud-account-chevron');
    expect(chevron).toHaveTextContent('⌄');
  });
});
