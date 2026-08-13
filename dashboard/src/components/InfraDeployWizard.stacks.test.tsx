import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { MemoryRouter } from 'react-router';

// jsdom has no scrollIntoView; the combobox calls it on option focus.
Element.prototype.scrollIntoView = Element.prototype.scrollIntoView ?? (() => {});
import { InfraDeployWizard } from './InfraDeployWizard';

// Reproduces the 2026-08-13 report "windows still cannot select all": on a
// Windows target the full Azure stack set (IIS + Caddy + Traefik) must be
// selectable together, survive the prune that runs on OS/cloud switches, and
// ride into the review step. Also pins the prune itself (the v0.28.200
// invisible-nginx fix) in both directions.

vi.mock('../api/client', () => ({
  api: {
    getCloudAccounts: vi.fn().mockResolvedValue([
      {
        account_id: 'acct-1',
        provider: 'azure',
        name: 'test-azure',
        status: 'active',
        region_default: 'eastus',
        validation_error: null,
      },
    ]),
    createDeployment: vi.fn().mockResolvedValue({ deployment_id: 'd-1', status: 'pending' }),
  },
}));
vi.mock('../api/testers', () => ({
  testersApi: { createTester: vi.fn() },
}));
vi.mock('../hooks/useToast', () => ({ useToast: () => vi.fn() }));

function renderWizard() {
  return render(
    <MemoryRouter>
      <InfraDeployWizard projectId="proj-1" onClose={() => {}} onCreated={() => {}} />
    </MemoryRouter>,
  );
}

async function walkToConfigure() {
  // Step 1: Kind — pick Target ("Server-under-test" card; default anyway).
  fireEvent.click(await screen.findByRole('button', { name: /server-under-test/i }));
  fireEvent.click(screen.getByRole('button', { name: /next:/i }));
  // Step 2: Cloud account — open the combobox and pick the mocked account
  // (canProceed requires accountId; options select on mousedown).
  fireEvent.focus(screen.getByRole('combobox'));
  fireEvent.mouseDown(await screen.findByRole('option', { name: /test-azure/i }));
  fireEvent.click(screen.getByRole('button', { name: /next:/i }));
  // Step 3: Region & OS — switch to Windows.
  fireEvent.click(await screen.findByRole('button', { name: /windows server/i }));
  fireEvent.click(screen.getByRole('button', { name: /next:/i }));
}

describe('InfraDeployWizard Windows stack selection', () => {
  beforeEach(() => vi.clearAllMocks());

  it('lets a Windows target select ALL Azure stacks (iis + caddy + traefik)', async () => {
    renderWizard();
    await walkToConfigure();

    // Prune fallback after the OS switch: IIS selected, others available.
    const iis = await screen.findByRole('button', { name: 'IIS' });
    const caddy = screen.getByRole('button', { name: 'Caddy' });
    const traefik = screen.getByRole('button', { name: 'Traefik' });
    expect(iis.className).toContain('border-cyan-700');

    fireEvent.click(caddy);
    fireEvent.click(traefik);

    expect(iis.className).toContain('border-cyan-700');
    expect(caddy.className).toContain('border-cyan-700');
    expect(traefik.className).toContain('border-cyan-700');
    // Linux-only stacks must not even render on Windows.
    expect(screen.queryByRole('button', { name: 'nginx' })).toBeNull();
  });

  it('prunes the Linux default when switching to Windows and restores sanity switching back', async () => {
    renderWizard();
    await walkToConfigure();

    // nginx (the Linux default) must NOT have survived the switch.
    expect(screen.queryByRole('button', { name: 'nginx' })).toBeNull();
    const iis = await screen.findByRole('button', { name: 'IIS' });
    expect(iis.className).toContain('border-cyan-700');
  });
});
