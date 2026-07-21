// CreateSdkEndpointDialog: form validation (name / absolute-URL / token /
// route), the write-only password token field, and a successful submit sending
// the typed create body.

import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CreateSdkEndpointDialog } from './CreateSdkEndpointDialog';

const createSdkEndpoint = vi.fn(() => Promise.resolve({}));

vi.mock('../api/client', () => ({
  errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  api: {
    createSdkEndpoint: (...a: unknown[]) => createSdkEndpoint(...(a as [])),
  },
}));

const addToast = vi.fn();
vi.mock('../hooks/useToast', () => ({ useToast: () => addToast }));

function renderDialog() {
  const onClose = vi.fn();
  const onCreated = vi.fn();
  render(<CreateSdkEndpointDialog projectId="p-1" onClose={onClose} onCreated={onCreated} />);
  return { onClose, onCreated };
}

describe('CreateSdkEndpointDialog', () => {
  afterEach(() => vi.clearAllMocks());

  it('renders the token as a write-only password field defaulting the route', () => {
    renderDialog();
    const token = screen.getByLabelText(/LagHound token/i) as HTMLInputElement;
    expect(token.type).toBe('password');
    const route = screen.getByLabelText(/Probe route/i) as HTMLInputElement;
    expect(route.value).toBe('/laghound/echo');
    // Submit is disabled until required fields are valid.
    expect(screen.getByRole('button', { name: 'Register endpoint' })).toBeDisabled();
  });

  it('rejects a non-absolute URL and keeps submit disabled', async () => {
    const user = userEvent.setup();
    renderDialog();
    await user.type(screen.getByLabelText('Name'), 'Checkout');
    await user.type(screen.getByLabelText('Target URL'), 'not-a-url');
    await user.type(screen.getByLabelText(/LagHound token/i), 'secret-token');
    expect(screen.getByText('Must be an absolute http(s) URL.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Register endpoint' })).toBeDisabled();
    expect(createSdkEndpoint).not.toHaveBeenCalled();
  });

  it('rejects a route that does not start with /', async () => {
    const user = userEvent.setup();
    renderDialog();
    const route = screen.getByLabelText(/Probe route/i);
    await user.clear(route);
    await user.type(route, 'laghound/echo');
    expect(screen.getByText(/Must be an absolute path beginning with/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Register endpoint' })).toBeDisabled();
  });

  it('submits the typed create body when the form is valid', async () => {
    const user = userEvent.setup();
    const { onCreated } = renderDialog();
    await user.type(screen.getByLabelText('Name'), 'Checkout API');
    await user.type(screen.getByLabelText('Target URL'), 'https://api.customer.com');
    await user.type(screen.getByLabelText(/LagHound token/i), 'lh-secret');

    const submit = screen.getByRole('button', { name: 'Register endpoint' });
    await waitFor(() => expect(submit).toBeEnabled());
    await user.click(submit);

    await waitFor(() =>
      expect(createSdkEndpoint).toHaveBeenCalledWith('p-1', {
        name: 'Checkout API',
        url: 'https://api.customer.com',
        token: 'lh-secret',
        route: '/laghound/echo',
        description: undefined,
      }),
    );
    expect(onCreated).toHaveBeenCalled();
  });
});
