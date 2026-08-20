// #782 P1 — URL sets on the probe page. The pure reductions are pinned in
// lib/probe-set.test.ts; this covers the page behaviour those reductions feed:
//
//  - the watchlist checkboxes assemble a set and "Probe set now" launches it as
//    ONE config carrying endpoint.hosts[] (→ one run, repeated tester --target);
//  - the multi-URL textarea takes a newline-separated paste and does the same;
//  - the single-URL flow is untouched — the plain input, Enter-to-run, and a
//    payload with no `hosts` key at all.
//
// The last point is the one that matters most: sets are additive, and a regression
// that turned every ordinary probe into a one-member set would be invisible in
// the UI and only show up in the stored config shape.

import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TestConfig, TestConfigListItem, TestRun } from '../api/types';
import { DiagnosticsPage } from './DiagnosticsPage';

const mocks = vi.hoisted(() => ({
  createConfig: vi.fn(),
  launchConfig: vi.fn(),
  listConfigs: vi.fn(),
  list: vi.fn(),
  listSchedules: vi.fn(),
  getAttempts: vi.fn(),
  getConfig: vi.fn(),
  deleteConfig: vi.fn(),
  createSchedule: vi.fn(),
  updateSchedule: vi.fn(),
  listTesters: vi.fn(),
  startTester: vi.fn(),
  addToast: vi.fn(),
}));

vi.mock('../features/runs/api', () => ({ runsApi: mocks }));
vi.mock('../api/testers', () => ({ testersApi: mocks }));
vi.mock('../hooks/useProject', () => ({ useProject: () => ({ projectId: 'project-1' }) }));
vi.mock('../hooks/useToast', () => ({ useToast: () => mocks.addToast }));
// The page polls the tester list on a timer; the test drives it directly.
vi.mock('../hooks/usePolling', () => ({ usePolling: vi.fn() }));

const ONE_HOUR_AGO = new Date(Date.now() - 60 * 60 * 1000).toISOString();

function watchConfig(id: string, host: string): TestConfigListItem {
  return {
    id,
    project_id: 'project-1',
    name: `Diag: ${host} (Quick)`,
    test_kind: 'url_probe',
    endpoint: { kind: 'network', host: `https://${host}/` },
    endpoint_kind: 'network',
    modes: ['dns', 'tcp', 'tls', 'http2'],
    has_methodology: false,
    created_at: ONE_HOUR_AGO,
    updated_at: ONE_HOUR_AGO,
  } as TestConfigListItem;
}

function watchRun(id: string, configId: string, host: string): TestRun {
  return {
    id,
    test_config_id: configId,
    project_id: 'project-1',
    status: 'completed',
    started_at: ONE_HOUR_AGO,
    finished_at: ONE_HOUR_AGO,
    success_count: 4,
    failure_count: 0,
    error_message: null,
    artifact_id: null,
    tester_id: null,
    worker_id: null,
    last_heartbeat: null,
    created_at: ONE_HOUR_AGO,
    config_name: `Diag: ${host} (Quick)`,
    endpoint_kind: 'network',
    test_kind: 'url_probe',
    modes: ['dns', 'tcp', 'tls', 'http2'],
  };
}

const HOSTS = ['a.example.com', 'b.example.com', 'c.example.com'];
const configs = HOSTS.map((h, i) => watchConfig(`config-${i}`, h));
const runs = HOSTS.map((h, i) => watchRun(`run-${i}`, `config-${i}`, h));

function renderPage(entry = '/projects/project-1/probe') {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[entry]}>
        <DiagnosticsPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The endpoint block of the single config the page created. */
function createdEndpoint() {
  expect(mocks.createConfig).toHaveBeenCalledTimes(1);
  return mocks.createConfig.mock.calls[0][1].endpoint;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.listConfigs.mockResolvedValue(configs);
  mocks.list.mockResolvedValue(runs);
  mocks.listSchedules.mockResolvedValue([]);
  mocks.getAttempts.mockResolvedValue([]);
  mocks.listTesters.mockResolvedValue([]);
  mocks.createConfig.mockImplementation((projectId: string, config: TestConfig) =>
    Promise.resolve({
      ...config,
      id: 'created-config',
      project_id: projectId,
      methodology: null,
      baseline_run_id: null,
      created_by: null,
      created_at: ONE_HOUR_AGO,
      updated_at: ONE_HOUR_AGO,
    }));
  mocks.launchConfig.mockResolvedValue({
    ...watchRun('launched-run-id-0000', 'created-config', 'a.example.com'),
    status: 'queued',
  });
});

// ── Watchlist multi-select ────────────────────────────────────────────────

describe('watchlist multi-select → probe set now', () => {
  it('ticking rows launches ONE config carrying every selected URL', async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole('checkbox', { name: 'Select a.example.com for a URL set' });
    await user.click(screen.getByRole('checkbox', { name: 'Select a.example.com for a URL set' }));
    await user.click(screen.getByRole('checkbox', { name: 'Select c.example.com for a URL set' }));

    const bar = screen.getByRole('region', { name: 'URL set actions' });
    expect(within(bar).getByText('2 URLs selected')).toBeInTheDocument();

    await user.click(within(bar).getByRole('button', { name: 'Probe set now' }));

    await waitFor(() => expect(mocks.createConfig).toHaveBeenCalledTimes(1));
    expect(createdEndpoint()).toEqual({
      kind: 'network',
      host: 'https://a.example.com/',
      hosts: ['https://a.example.com/', 'https://c.example.com/'],
    });
    // ONE run for the whole set — the run list caps at 200 newest, so N rows
    // per probe is exactly what sets exist to avoid.
    await waitFor(() => expect(mocks.launchConfig).toHaveBeenCalledTimes(1));
  });

  it('the action bar only exists while something is selected', async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole('checkbox', { name: 'Select a.example.com for a URL set' });
    expect(screen.queryByRole('region', { name: 'URL set actions' })).not.toBeInTheDocument();

    const box = screen.getByRole('checkbox', { name: 'Select b.example.com for a URL set' });
    await user.click(box);
    expect(screen.getByRole('region', { name: 'URL set actions' })).toBeInTheDocument();

    await user.click(box);
    expect(screen.queryByRole('region', { name: 'URL set actions' })).not.toBeInTheDocument();
  });

  it('Clear drops the whole selection', async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole('checkbox', { name: 'Select a.example.com for a URL set' });
    await user.click(screen.getByRole('checkbox', { name: 'Select a.example.com for a URL set' }));
    await user.click(screen.getByRole('checkbox', { name: 'Select b.example.com for a URL set' }));

    const bar = screen.getByRole('region', { name: 'URL set actions' });
    await user.click(within(bar).getByRole('button', { name: 'Clear' }));

    expect(screen.queryByRole('region', { name: 'URL set actions' })).not.toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: 'Select a.example.com for a URL set' })).not.toBeChecked();
  });

  it('select-all covers exactly the rows on this page', async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole('checkbox', { name: 'Select a.example.com for a URL set' });
    await user.click(screen.getByRole('checkbox', { name: /Select all 3 on this page/ }));

    const bar = screen.getByRole('region', { name: 'URL set actions' });
    expect(within(bar).getByText('3 URLs selected')).toBeInTheDocument();
  });

  it('"Edit as list" loads the selection into the multi-URL box instead of probing', async () => {
    const user = userEvent.setup();
    renderPage();

    await screen.findByRole('checkbox', { name: 'Select a.example.com for a URL set' });
    await user.click(screen.getByRole('checkbox', { name: 'Select a.example.com for a URL set' }));
    await user.click(screen.getByRole('checkbox', { name: 'Select b.example.com for a URL set' }));

    const bar = screen.getByRole('region', { name: 'URL set actions' });
    await user.click(within(bar).getByRole('button', { name: 'Edit as list' }));

    const textarea = screen.getByLabelText('URLs to probe together, one per line');
    expect(textarea).toHaveValue('a.example.com\nb.example.com');
    expect(mocks.createConfig).not.toHaveBeenCalled();
  });
});

// ── Multi-URL textarea ────────────────────────────────────────────────────

describe('multi-URL entry', () => {
  it('a newline-separated paste becomes one set config', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: 'URL set' }));
    const textarea = screen.getByLabelText('URLs to probe together, one per line');
    await user.click(textarea);
    await user.paste('one.example.com\ntwo.example.com\nthree.example.com');

    expect(screen.getByText(/3 URLs — probed together in one run/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Probe 3 URLs together' }));

    await waitFor(() => expect(mocks.createConfig).toHaveBeenCalledTimes(1));
    expect(createdEndpoint()).toEqual({
      kind: 'network',
      host: 'https://one.example.com/',
      hosts: [
        'https://one.example.com/',
        'https://two.example.com/',
        'https://three.example.com/',
      ],
    });
  });

  it('names the lines it will not probe rather than dropping them quietly', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: 'URL set' }));
    const textarea = screen.getByLabelText('URLs to probe together, one per line');
    await user.click(textarea);
    await user.paste('good.example.com\nftp://files.example.com\n###');

    expect(screen.getByText(/1 URL$/)).toBeInTheDocument();
    expect(screen.getByText(/2 unusable/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Run diagnostic' }));

    await waitFor(() => expect(mocks.createConfig).toHaveBeenCalledTimes(1));
    // The bad lines never reach the config…
    expect(createdEndpoint()).toEqual({ kind: 'network', host: 'https://good.example.com/' });
    // …and the user is told, before the run, that they were skipped.
    expect(mocks.addToast).toHaveBeenCalledWith(
      'info',
      expect.stringContaining('Skipped 2 unusable entries'),
    );
  });

  it('refuses to look ready when nothing in the box is probeable', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('button', { name: 'URL set' }));
    await user.click(screen.getByLabelText('URLs to probe together, one per line'));
    await user.paste('###\nftp://x.example.com');

    expect(screen.getByRole('button', { name: 'Run diagnostic' })).toBeDisabled();
  });

  it('switching modes carries the same URLs across', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByLabelText('URL or hostname to test'), 'a.example.com b.example.com');
    await user.click(screen.getByRole('button', { name: 'URL set' }));
    expect(screen.getByLabelText('URLs to probe together, one per line'))
      .toHaveValue('a.example.com\nb.example.com');

    await user.click(screen.getByRole('button', { name: 'Single URL' }));
    expect(screen.getByLabelText('URL or hostname to test'))
      .toHaveValue('a.example.com b.example.com');
  });
});

// ── The single-URL flow is untouched ──────────────────────────────────────

describe('single-URL flow (unchanged)', () => {
  it('opens on the single-line input, not the set textarea', async () => {
    renderPage();

    expect(screen.getByLabelText('URL or hostname to test')).toBeInTheDocument();
    expect(screen.queryByLabelText('URLs to probe together, one per line')).not.toBeInTheDocument();
  });

  it('Enter still runs, and the payload carries NO hosts key', async () => {
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByLabelText('URL or hostname to test'), 'solo.example.com{Enter}');

    await waitFor(() => expect(mocks.createConfig).toHaveBeenCalledTimes(1));
    const endpoint = createdEndpoint();
    expect(endpoint).toEqual({ kind: 'network', host: 'https://solo.example.com/' });
    expect(endpoint).not.toHaveProperty('hosts');
    expect(mocks.createConfig.mock.calls[0][1].name).toBe('Diag: solo.example.com (Quick)');
  });

  it('opens straight into the set box when the shared link carries several hosts', async () => {
    renderPage('/projects/project-1/probe?host=a.example.com,b.example.com');

    expect(screen.getByLabelText('URLs to probe together, one per line'))
      .toHaveValue('a.example.com\nb.example.com');
  });
});
