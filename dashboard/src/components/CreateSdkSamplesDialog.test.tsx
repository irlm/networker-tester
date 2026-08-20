// CreateSdkSamplesDialog: the create request the owner asked for — "all
// languages on one server, or separated (one server per language)" — plus the
// reuse-first default. What matters is the BODY that reaches the API and the
// cost summary the user reads before clicking, so both are asserted here.

import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { CreateSdkSamplesDialog } from './CreateSdkSamplesDialog';
import type { SdkSampleCreateBody, SdkSampleStatus } from '../api/types';

const createSdkSamples = vi.fn(() =>
  Promise.resolve({
    shape: 'consolidated',
    reused: [],
    deployments: [],
    pending_registration: [],
    servers_provisioned: 1,
    servers_avoided: 0,
  }),
);
const getSdkSamples = vi.fn(() =>
  Promise.resolve({
    catalog: { prefix_default: '/laghound', route_default: '/laghound/echo', samples: [] },
    samples: [],
    cost_preview: {
      provider: 'azure',
      region: 'eastus',
      vm_size: 'Standard_B2s',
      hourly_usd: 0.1,
      monthly_usd: 72,
      note: 'per server, always-on',
    },
  }),
);

vi.mock('../api/client', () => ({
  errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  api: {
    createSdkSamples: (...a: unknown[]) => createSdkSamples(...(a as [])),
    getSdkSamples: (...a: unknown[]) => getSdkSamples(...(a as [])),
    getCloudAccounts: () =>
      Promise.resolve([
        {
          account_id: 'acc-1',
          name: 'test-azure',
          provider: 'azure',
          region_default: 'eastus',
          personal: false,
          status: 'active',
          last_validated: null,
          validation_error: null,
        },
      ]),
  },
}));
vi.mock('../hooks/useToast', () => ({ useToast: () => vi.fn() }));
vi.mock('../hooks/useDockerProvider', async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useDockerProvider: () => false,
}));

function sample(over: Partial<SdkSampleStatus> & { language: string }): SdkSampleStatus {
  return {
    label: over.language,
    runtime: 'runtime',
    description: 'desc',
    port: 8101,
    url: null,
    route: '/laghound/echo',
    state: 'none',
    recommended_action: 'create',
    reason: 'Nothing deployed for this language yet.',
    reusable: false,
    current_version: '1.0.0',
    deployed_version: null,
    deployment_id: null,
    deployment_name: null,
    deployment_status: null,
    host: null,
    provider: null,
    region: null,
    vm_size: null,
    consolidated: null,
    samples_on_host: null,
    sdk_endpoint_id: null,
    ...over,
  };
}

const SAMPLES = [
  sample({ language: 'go' }),
  sample({ language: 'python' }),
  sample({
    language: 'rust',
    state: 'current',
    recommended_action: 'reuse',
    reusable: true,
    deployed_version: '1.0.0',
    deployment_id: 'd-1',
  }),
];

function open(samples = SAMPLES) {
  const onCreated = vi.fn();
  render(
    <CreateSdkSamplesDialog
      projectId="p-1"
      samples={samples}
      onClose={() => {}}
      onCreated={onCreated}
    />,
  );
  return { onCreated };
}

function submittedBody(): SdkSampleCreateBody {
  const calls = (createSdkSamples as unknown as { mock: { calls: unknown[][] } }).mock.calls;
  return calls[0][1] as SdkSampleCreateBody;
}

describe('CreateSdkSamplesDialog', () => {
  beforeEach(() => vi.clearAllMocks());

  it('defaults to consolidated — the cheap shape — and to reusing what exists', async () => {
    const user = userEvent.setup();
    open();
    const submit = await screen.findByRole('button', { name: /Deploy 1 server/ });
    await user.click(submit);
    await waitFor(() => expect(createSdkSamples).toHaveBeenCalled());

    const body = submittedBody();
    expect(body.shape).toBe('consolidated');
    expect(body.reuse_existing).toBe(true);
    // rust is already current, so it is not pre-selected — go + python are.
    expect(body.languages).toEqual(['go', 'python']);
    expect(body.provider).toBe('azure');
  });

  it('separated sends the same languages with the per-language shape', async () => {
    const user = userEvent.setup();
    open();
    await user.click(screen.getByRole('button', { name: /Separated/ }));
    await user.click(await screen.findByRole('button', { name: /Deploy 2 servers/ }));
    await waitFor(() => expect(createSdkSamples).toHaveBeenCalled());
    expect(submittedBody().shape).toBe('separated');
  });

  it('prices what it will start: 1 server consolidated, N separated', async () => {
    const user = userEvent.setup();
    open();
    await waitFor(() => expect(screen.getByText(/\$0\.100\/h · \$72\.00\/mo/)).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: /Separated/ }));
    await waitFor(() => expect(screen.getByText(/\$0\.200\/h · \$144\.00\/mo/)).toBeInTheDocument());
  });

  it('adding a reusable language costs nothing and says what it avoids', async () => {
    const user = userEvent.setup();
    open();
    await user.click(screen.getByLabelText(/rust/));
    await waitFor(() => expect(screen.getByText(/avoids \$72\.00\/mo/)).toBeInTheDocument());
    // Still one server: rust is reused, not provisioned.
    await user.click(screen.getByRole('button', { name: /Deploy 1 server · reuse 1/ }));
    await waitFor(() => expect(createSdkSamples).toHaveBeenCalled());
    expect(submittedBody().languages).toEqual(['go', 'python', 'rust']);
  });

  it('unticking reuse provisions the existing language too, explicitly', async () => {
    const user = userEvent.setup();
    open();
    await user.click(screen.getByLabelText(/rust/));
    await user.click(screen.getByLabelText(/Reuse servers that already run/));
    await user.click(await screen.findByRole('button', { name: /Deploy 1 server$/ }));
    await waitFor(() => expect(createSdkSamples).toHaveBeenCalled());
    expect(submittedBody().reuse_existing).toBe(false);
  });

  it('a reuse-only request provisions nothing and never sends a provider', async () => {
    const user = userEvent.setup();
    open();
    // Drop the two undeployed languages, leaving only the reusable one.
    await user.click(screen.getByLabelText(/go/));
    await user.click(screen.getByLabelText(/python/));
    await user.click(screen.getByLabelText(/rust/));
    await waitFor(() =>
      expect(screen.getByText(/every selected language already has a usable server/)).toBeInTheDocument(),
    );
    await user.click(screen.getByRole('button', { name: /Reuse 1 existing/ }));
    await waitFor(() => expect(createSdkSamples).toHaveBeenCalled());

    const body = submittedBody();
    expect(body.languages).toEqual(['rust']);
    expect(body.provider).toBeUndefined();
    expect(body.vm_size).toBeUndefined();
  });
});
