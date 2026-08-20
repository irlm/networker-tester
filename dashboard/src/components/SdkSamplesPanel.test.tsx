// SdkSamplesPanel: the page's answer to "we can see the samples but we cannot
// create". Every row must carry an honest state and exactly one action —
// reuse when something usable exists (the cheap default), update when the
// deployed sample is behind the catalog, redeploy when it is dead, deploy when
// there is nothing. A stale or unreachable sample must NEVER read as usable.

import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router';
import { SdkSamplesPanel } from './SdkSamplesPanel';
import type { SdkSampleStatus } from '../api/types';

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

function renderPanel(samples: SdkSampleStatus[], isOperator = true) {
  const onDeploy = vi.fn();
  const onReuse = vi.fn();
  const onUpdate = vi.fn();
  render(
    <MemoryRouter>
      <SdkSamplesPanel
        projectId="p-1"
        samples={samples}
        isOperator={isOperator}
        onDeploy={onDeploy}
        onReuse={onReuse}
        onUpdate={onUpdate}
      />
    </MemoryRouter>,
  );
  return { onDeploy, onReuse, onUpdate };
}

/** The <article> for one language. */
function row(label: string): HTMLElement {
  const heading = screen.getByRole('heading', { name: label });
  const article = heading.closest('article');
  if (!article) throw new Error(`no row for ${label}`);
  return article;
}

describe('SdkSamplesPanel', () => {
  it('offers Deploy when nothing is deployed', async () => {
    const user = userEvent.setup();
    const { onDeploy } = renderPanel([sample({ language: 'go' })]);
    expect(within(row('go')).getByText('not deployed')).toBeInTheDocument();
    await user.click(within(row('go')).getByRole('button', { name: 'Deploy' }));
    expect(onDeploy).toHaveBeenCalledWith('go');
  });

  it('offers Reuse — not a second deployment — when a current sample exists', async () => {
    const user = userEvent.setup();
    const { onReuse, onDeploy } = renderPanel([
      sample({
        language: 'go',
        state: 'current',
        recommended_action: 'reuse',
        reusable: true,
        deployed_version: '1.0.0',
        deployment_id: 'd-1',
        url: 'http://10.0.0.4:8105',
      }),
    ]);
    expect(within(row('go')).getByText('deployed · current')).toBeInTheDocument();
    await user.click(within(row('go')).getByRole('button', { name: 'Reuse' }));
    expect(onReuse).toHaveBeenCalledWith('go');
    expect(onDeploy).not.toHaveBeenCalled();
  });

  it('says "in use" instead of a dead Reuse button when it is already registered', () => {
    renderPanel([
      sample({
        language: 'go',
        state: 'current',
        recommended_action: 'reuse',
        reusable: true,
        deployed_version: '1.0.0',
        deployment_id: 'd-1',
        sdk_endpoint_id: 'sdk-1',
      }),
    ]);
    expect(within(row('go')).getByText('in use')).toBeInTheDocument();
    expect(within(row('go')).getByText('registered')).toBeInTheDocument();
    expect(within(row('go')).queryByRole('button', { name: 'Reuse' })).not.toBeInTheDocument();
  });

  it('shows deployed→current versions and offers Update when the sample is outdated', async () => {
    const user = userEvent.setup();
    const { onUpdate } = renderPanel([
      sample({
        language: 'rust',
        state: 'outdated',
        recommended_action: 'update',
        reusable: true,
        deployed_version: '0.9.0',
        deployment_id: 'd-2',
        sdk_endpoint_id: 'sdk-2',
        reason: 'Deployed sample runs SDK 0.9.0; the current sample is 1.0.0.',
      }),
    ]);
    const r = row('rust');
    expect(within(r).getByText('deployed · outdated')).toBeInTheDocument();
    expect(within(r).getByText('0.9.0 → 1.0.0')).toBeInTheDocument();
    await user.click(within(r).getByRole('button', { name: /Update → 1\.0\.0/ }));
    expect(onUpdate).toHaveBeenCalledWith('rust');
  });

  it('an unreachable sample reads as unreachable and offers Redeploy, never Reuse', async () => {
    const user = userEvent.setup();
    const { onDeploy, onReuse } = renderPanel([
      sample({
        language: 'js',
        state: 'unhealthy',
        recommended_action: 'redeploy',
        reusable: false,
        deployment_id: 'd-3',
        reason: '10.0.0.9:8102 did not answer /health — the sample is not serving.',
      }),
    ]);
    const r = row('js');
    expect(within(r).getByText('unreachable')).toBeInTheDocument();
    expect(within(r).queryByRole('button', { name: 'Reuse' })).not.toBeInTheDocument();
    await user.click(within(r).getByRole('button', { name: 'Redeploy' }));
    expect(onDeploy).toHaveBeenCalledWith('js');
    expect(onReuse).not.toHaveBeenCalled();
  });

  it('an in-flight deployment offers no action at all', () => {
    renderPanel([
      sample({
        language: 'csharp',
        state: 'deploying',
        recommended_action: 'wait',
        deployment_id: 'd-4',
        deployment_status: 'running',
      }),
    ]);
    const r = row('csharp');
    expect(within(r).getByText('deploying')).toBeInTheDocument();
    expect(within(r).getByText('Deploying…')).toBeInTheDocument();
    expect(within(r).queryByRole('button')).not.toBeInTheDocument();
  });

  it('viewers get the states but no actions', () => {
    renderPanel(
      [sample({ language: 'go', state: 'current', recommended_action: 'reuse', reusable: true })],
      false,
    );
    expect(screen.getByText('deployed · current')).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('surfaces where a consolidated sample runs and who it shares with', () => {
    renderPanel([
      sample({
        language: 'go',
        state: 'current',
        recommended_action: 'reuse',
        reusable: true,
        deployed_version: '1.0.0',
        deployment_id: 'd-1',
        deployment_name: 'sdk-go-js-ab12',
        provider: 'azure',
        region: 'eastus',
        vm_size: 'Standard_B2s',
        consolidated: true,
        samples_on_host: 2,
        url: 'http://20.1.2.3:8105',
      }),
    ]);
    const r = row('go');
    expect(within(r).getByText('http://20.1.2.3:8105')).toBeInTheDocument();
    expect(within(r).getByText(/azure/)).toBeInTheDocument();
    expect(within(r).getByText(/shares this server with 1 other sample/)).toBeInTheDocument();
    expect(within(r).getByRole('link', { name: /sdk-go-js-ab12/ })).toHaveAttribute(
      'href',
      '/projects/p-1/deploy/d-1',
    );
  });

  it('counts what is deployed and what is behind', () => {
    renderPanel([
      sample({ language: 'go', state: 'current', recommended_action: 'reuse', reusable: true }),
      sample({ language: 'rust', state: 'outdated', recommended_action: 'update', reusable: true }),
      sample({ language: 'python' }),
    ]);
    expect(screen.getByText(/2 of 3 deployed/)).toBeInTheDocument();
    expect(screen.getByText(/1 outdated/)).toBeInTheDocument();
  });
});
