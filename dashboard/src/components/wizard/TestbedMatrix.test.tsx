import { render, screen } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { MemoryRouter } from 'react-router';
import type { TesterRow } from '../../api/testers';
import { TestbedMatrix } from './TestbedMatrix';
import { makeTestbed } from './testbed-constants';

// jsdom has no scrollIntoView; the account combobox calls it on option focus.
Element.prototype.scrollIntoView = Element.prototype.scrollIntoView ?? (() => {});

// #793 P3: this file counted a runner "online" on power_state alone, so a
// running VM whose agent was dark was countable AND pinnable — the run then
// queued forever. "Online" is now the STRICT shared predicate
// (lib/tester-readiness: running VM + connected agent) everywhere.

function tester(over: Partial<TesterRow>): TesterRow {
  return {
    tester_id: 't-1',
    name: 'runner-1',
    cloud: 'Azure',
    region: 'eastus',
    power_state: 'running',
    allocation: 'idle',
    agent_status: 'online',
    ...over,
  } as TesterRow;
}

const listTesters = vi.fn();
vi.mock('../../api/testers', () => ({
  testersApi: { listTesters: (...args: unknown[]) => listTesters(...args) },
}));
vi.mock('../../api/client', () => ({
  api: { getCloudAccounts: vi.fn().mockResolvedValue([]) },
}));

function renderMatrix(props: Partial<Parameters<typeof TestbedMatrix>[0]> = {}) {
  return render(
    <MemoryRouter>
      <TestbedMatrix
        projectId="proj-1"
        testbeds={[makeTestbed(0, 'Azure', 'linux', ['nginx'])]}
        onTestbedsChange={() => {}}
        runnerMode="specific"
        onRunnerModeChange={() => {}}
        selectedTesterId={null}
        onTesterIdChange={() => {}}
        proxyWarning={false}
        {...props}
      />
    </MemoryRouter>,
  );
}

describe('TestbedMatrix — strict runner-online definition (#793 P3)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listTesters.mockResolvedValue([
      tester({ tester_id: 't-ok', name: 'runner-ok', agent_status: 'online' }),
      tester({ tester_id: 't-dark', name: 'runner-dark', agent_status: 'offline' }),
      tester({ tester_id: 't-stopped', name: 'runner-stopped', power_state: 'stopped', agent_status: null }),
    ]);
  });

  it('counts only running+agent-online runners as online', async () => {
    renderMatrix();
    // 3 rows, but only runner-ok satisfies the strict predicate.
    expect(await screen.findByText('1 idle / 1 online')).toBeInTheDocument();
  });

  it('disables a running runner whose agent is dark and says why', async () => {
    renderMatrix();
    const radios = await screen.findAllByRole('radio');
    const byValue = Object.fromEntries(radios.map(r => [(r as HTMLInputElement).value, r]));
    expect(byValue['t-ok']).toBeEnabled();
    expect(byValue['t-dark']).toBeDisabled();
    expect(byValue['t-stopped']).toBeDisabled();
    // The status chip is honest instead of a green "idle" on a disabled row.
    expect(screen.getByText('agent offline')).toBeInTheDocument();
  });
});

describe('TestbedMatrix — no "Use existing VM" in the benchmark wizards (#793 P2-3)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listTesters.mockResolvedValue([]);
  });

  it('never offers the control its hosts silently discard', async () => {
    renderMatrix();
    // The testbed row renders (region select present) but the existing-VM
    // checkbox — whose value buildComparisonCells never reads — does not.
    expect(await screen.findByText('Reverse Proxies')).toBeInTheDocument();
    expect(screen.queryByText(/use existing vm/i)).not.toBeInTheDocument();
  });
});
