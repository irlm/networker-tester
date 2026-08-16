import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { MemoryRouter } from 'react-router';

Element.prototype.scrollIntoView = Element.prototype.scrollIntoView ?? (() => {});
import { InfraDeployWizard } from './InfraDeployWizard';
import { api } from '../api/client';
import { testersApi } from '../api/testers';

// The feature-flagged Docker (local) provider in the infra wizard: the option
// card is offered on the cloud step only when /api/version reports
// docker_provider, and both kinds submit account-less docker payloads.

const versionInfo = { docker_provider: true };

vi.mock('../api/client', () => ({
  api: {
    getCloudAccounts: vi.fn().mockResolvedValue([]),
    getVersionInfo: vi.fn(() => Promise.resolve(versionInfo)),
    createDeployment: vi.fn().mockResolvedValue({ deployment_id: 'd-1', status: 'pending' }),
  },
}));
vi.mock('../api/testers', () => ({
  testersApi: { createTester: vi.fn().mockResolvedValue({ tester_id: 't-1' }) },
}));
vi.mock('../hooks/useToast', () => ({ useToast: () => vi.fn() }));

function renderWizard(kind: 'target' | 'runner' = 'target') {
  return render(
    <MemoryRouter>
      <InfraDeployWizard projectId="proj-1" initialKind={kind} onClose={() => {}} onCreated={() => {}} />
    </MemoryRouter>,
  );
}

describe('InfraDeployWizard — Docker (local) provider', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    versionInfo.docker_provider = true;
  });

  it('does not offer Docker (local) when the flag is off', async () => {
    versionInfo.docker_provider = false;
    renderWizard();
    fireEvent.click(await screen.findByRole('button', { name: /server-under-test/i }));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));
    await screen.findByRole('combobox');
    await waitFor(() => expect(api.getVersionInfo).toHaveBeenCalled());
    expect(screen.queryByRole('button', { name: /Docker \(local\)/i })).toBeNull();
    // No account and no docker → cannot proceed.
    expect(screen.getByRole('button', { name: /next:/i })).toBeDisabled();
  });

  it('deploys a docker target: one stack, no cloud_account_id, provider docker', async () => {
    renderWizard();
    fireEvent.click(await screen.findByRole('button', { name: /server-under-test/i }));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));

    fireEvent.click(await screen.findByRole('button', { name: /Docker \(local\)/i }));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));

    // Region step: pinned to local / container; Windows disabled.
    expect(screen.getByDisplayValue('local')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /windows server/i })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));

    // Configure: nginx pre-selected; single-select — picking Caddy replaces it.
    const nginx = await screen.findByRole('button', { name: 'nginx' });
    expect(nginx.className).toContain('border-cyan-700');
    fireEvent.click(screen.getByRole('button', { name: 'Caddy' }));
    expect(screen.getByRole('button', { name: 'Caddy' }).className).toContain('border-cyan-700');
    expect(screen.getByRole('button', { name: 'nginx' }).className).not.toContain('border-cyan-700');
    // No reference APIs / existing VM controls on docker targets.
    expect(screen.queryByText(/Reference APIs/)).toBeNull();
    expect(screen.queryByText(/Use existing VM/)).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));

    // Review + deploy.
    expect(await screen.findByText('Docker (local)')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /^deploy/i }));

    await waitFor(() => expect(api.createDeployment).toHaveBeenCalled());
    const [, , config] = (api.createDeployment as unknown as { mock: { calls: unknown[][] } }).mock.calls[0] as [string, string, Record<string, unknown>];
    expect(config.cloud_account_id).toBeUndefined();
    const endpoints = config.endpoints as Record<string, unknown>[];
    expect(endpoints).toHaveLength(1);
    expect(endpoints[0].provider).toBe('docker');
    expect(endpoints[0].http_stacks).toEqual(['caddy']);
    expect(endpoints[0].languages).toBeUndefined();
  });

  it('creates a docker runner: cloud docker, region local, no account', async () => {
    renderWizard('runner');
    fireEvent.click(await screen.findByRole('button', { name: /load-generator/i }));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));
    fireEvent.click(await screen.findByRole('button', { name: /Docker \(local\)/i }));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));
    // Runner name step (auto-filled from region), then review.
    await waitFor(() => expect((screen.getByPlaceholderText(/-runner-01/) as HTMLInputElement).value).not.toBe(''));
    fireEvent.click(screen.getByRole('button', { name: /next:/i }));
    fireEvent.click(await screen.findByRole('button', { name: /create runner/i }));

    await waitFor(() => expect(testersApi.createTester).toHaveBeenCalled());
    const [, body] = (testersApi.createTester as unknown as { mock: { calls: unknown[][] } }).mock.calls[0] as [string, Record<string, unknown>];
    expect(body.cloud).toBe('docker');
    expect(body.region).toBe('local');
    expect(body.vm_size).toBe('container');
    expect(body.requested_os).toBe('ubuntu-24.04');
    expect(body.cloud_account_id).toBeUndefined();
  });
});
