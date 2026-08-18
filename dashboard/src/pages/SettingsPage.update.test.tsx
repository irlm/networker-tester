// SettingsPage "deployed targets" update flow — regression tests for the
// silent-update defect (prod, 2026-08-17: clicking "update" on a deployed
// target twice showed no error while the endpoint stayed on the old version).
//
// The three ways an update outcome used to be swallowed:
//   1. a rejected POST toasted a generic line with the server's reason dropped;
//   2. the deploy_complete watcher looked only at the single LATEST live event,
//      so any unrelated event (heartbeat, run attempt) landing after
//      deploy_complete made the completion invisible — no toast, ever;
//   3. "completed" was trusted blindly — a deploy that exited 0 without the
//      endpoint actually picking up the new binary toasted success.

import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import { SettingsPage } from './SettingsPage';
import { resetRoleStores, setProjectRole } from '../test/rbac-helpers';
import { useLiveStore } from '../stores/liveStore';
import type { Deployment } from '../api/types';

const deployment = {
  deployment_id: 'dep-1',
  name: 'target-azure-eastus-nginx-x759',
  status: 'completed',
  config: {},
  provider_summary: 'azure / eastus',
  created_by: 'u-1',
  created_at: '2026-08-01T00:00:00Z',
  started_at: null,
  finished_at: '2026-08-01T00:10:00Z',
  endpoint_ips: ['20.1.2.3'],
  agent_id: null,
  error_message: null,
  log: null,
} as unknown as Deployment;

const getVersionInfo = vi.fn(() => Promise.resolve({
  dashboard_version: '0.28.231',
  tester_version: null,
  latest_release: '0.28.231',
  update_available: false,
  endpoints: [],
}));
const getDeployments = vi.fn(() => Promise.resolve([deployment]));
const getCloudConnections = vi.fn(() => Promise.resolve([]));
const updateEndpoint = vi.fn<(...a: unknown[]) => Promise<{ status: string }>>(
  () => Promise.resolve({ status: 'updating' }));
const getDeployment = vi.fn(() => Promise.resolve(deployment));
const checkDeployment = vi.fn(() => Promise.resolve({
  endpoints: [{ ip: '20.1.2.3', alive: true, version: '0.28.231', outdated: false }],
  latest_release: '0.28.231',
}));

vi.mock('../api/client', () => ({
  errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  api: {
    getVersionInfo: (...a: unknown[]) => getVersionInfo(...(a as [])),
    getDeployments: (...a: unknown[]) => getDeployments(...(a as [])),
    getCloudConnections: (...a: unknown[]) => getCloudConnections(...(a as [])),
    updateEndpoint: (...a: unknown[]) => updateEndpoint(...a),
    getDeployment: (...a: unknown[]) => getDeployment(...(a as [])),
    checkDeployment: (...a: unknown[]) => checkDeployment(...(a as [])),
  },
}));

const addToast = vi.fn();
vi.mock('../hooks/useToast', () => ({ useToast: () => addToast }));
vi.mock('../components/SystemHealthPanel', () => ({ default: () => null }));
vi.mock('../components/common/SettingsTabs', () => ({ SettingsTabs: () => null }));

function pushEvent(event: Record<string, unknown>) {
  act(() => {
    useLiveStore.getState().addEvent(event as never);
  });
}

async function renderAndClickUpdate() {
  render(
    <MemoryRouter>
      <SettingsPage />
    </MemoryRouter>,
  );
  await waitFor(() =>
    expect(screen.getByText('target-azure-eastus-nginx-x759')).toBeInTheDocument());
  await userEvent.click(screen.getByRole('button', { name: 'update' }));
}

describe('SettingsPage deployed-target update', () => {
  beforeEach(() => {
    setProjectRole('admin');
  });

  afterEach(() => {
    resetRoleStores();
    useLiveStore.setState({ events: [], deployLogs: {} });
    vi.clearAllMocks();
  });

  it('surfaces the server reason when the update POST is rejected', async () => {
    updateEndpoint.mockRejectedValueOnce(new Error('deployment not found'));
    await renderAndClickUpdate();

    await waitFor(() => expect(addToast).toHaveBeenCalledWith(
      'error', expect.stringContaining('deployment not found')));
    // The button must be clickable again, not wedged at "updating…".
    expect(screen.getByRole('button', { name: 'update' })).toBeEnabled();
  });

  it('toasts the recorded failure reason on deploy_complete{failed}', async () => {
    getDeployment.mockResolvedValueOnce({
      ...deployment,
      status: 'failed',
      error_message: 'install.sh exited with code 1',
    } as unknown as Deployment);
    await renderAndClickUpdate();
    await waitFor(() => expect(updateEndpoint).toHaveBeenCalled());

    pushEvent({ type: 'deploy_complete', deployment_id: 'dep-1', status: 'failed', seq: 10 });

    await waitFor(() => expect(addToast).toHaveBeenCalledWith(
      'error', expect.stringContaining('install.sh exited with code 1')));
  });

  it('sees deploy_complete even when an unrelated event lands after it', async () => {
    await renderAndClickUpdate();
    await waitFor(() => expect(updateEndpoint).toHaveBeenCalled());

    // deploy_complete followed by an unrelated heartbeat — the old watcher
    // inspected only events[events.length - 1] and missed the completion.
    pushEvent({ type: 'deploy_complete', deployment_id: 'dep-1', status: 'completed', seq: 11 });
    pushEvent({ type: 'agent_heartbeat', agent_id: 'a-1', seq: 12 });

    await waitFor(() => expect(addToast).toHaveBeenCalledWith('success', 'Update completed'));
    expect(checkDeployment).toHaveBeenCalledTimes(1);
  });

  it('reports an error when the deploy completed but the endpoint still runs the old version', async () => {
    checkDeployment.mockResolvedValueOnce({
      endpoints: [{ ip: '20.1.2.3', alive: true, version: '0.28.227', outdated: true }],
      latest_release: '0.28.231',
    });
    await renderAndClickUpdate();
    await waitFor(() => expect(updateEndpoint).toHaveBeenCalled());

    pushEvent({ type: 'deploy_complete', deployment_id: 'dep-1', status: 'completed', seq: 13 });

    await waitFor(() => expect(addToast).toHaveBeenCalledWith(
      'error', expect.stringContaining('still reports v0.28.227')));
    // Never a success toast for a lying "completed".
    expect(addToast).not.toHaveBeenCalledWith('success', 'Update completed');
  });

  it('ignores a stale deploy_complete left over from before the click', async () => {
    // Seed a completion from an EARLIER update of the same deployment.
    pushEvent({ type: 'deploy_complete', deployment_id: 'dep-1', status: 'completed', seq: 5 });
    await renderAndClickUpdate();
    await waitFor(() => expect(updateEndpoint).toHaveBeenCalled());

    // No new completion has arrived — the stale one must not be consumed.
    expect(checkDeployment).not.toHaveBeenCalled();
    expect(addToast).not.toHaveBeenCalledWith('success', 'Update completed');
  });
});
