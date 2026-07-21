import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { RotateKeyDialog } from './RotateKeyDialog';

function mockFetch(body: unknown) {
  return Promise.resolve({
    ok: true,
    status: 200,
    statusText: 'OK',
    headers: new Headers(),
    text: () => Promise.resolve(JSON.stringify(body)),
  } as unknown as Response);
}

const NEW_KEY = 'AbCdEfGhIjKlMnOpQrStUvWxYz0123456789AbCdEfGhIjKl';

describe('RotateKeyDialog', () => {
  beforeEach(() => {
    localStorage.setItem('token', 'test');
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('confirms first, then shows the new key ONCE with a copy button', async () => {
    const fetchMock = vi.fn(() =>
      mockFetch({
        agent_id: 'a-1',
        tester_id: 't-1',
        api_key: NEW_KEY,
        api_key_expires_at: null,
      }),
    );
    vi.stubGlobal('fetch', fetchMock);
    const writeText = vi.fn(() => Promise.resolve());
    vi.stubGlobal('navigator', { clipboard: { writeText } });

    render(
      <RotateKeyDialog
        projectId="p-1"
        testerId="t-1"
        testerName="eastus-1"
        onClose={() => {}}
      />,
    );

    // Stage 1: confirmation — the key is NOT shown yet.
    expect(screen.queryByDisplayValue(NEW_KEY)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /rotate key/i }));

    // Stage 2: the new key is revealed once.
    await waitFor(() =>
      expect(screen.getByDisplayValue(NEW_KEY)).toBeInTheDocument(),
    );
    expect(
      screen.getByText(/will not be shown again/i),
    ).toBeInTheDocument();

    // It POSTed to the rotate-key route.
    const url = fetchMock.mock.calls[0][0] as string;
    expect(url).toContain('/projects/p-1/testers/t-1/rotate-key');

    // Copy button puts the key on the clipboard.
    fireEvent.click(screen.getByRole('button', { name: /copy/i }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(NEW_KEY));
  });

  it('surfaces an error and does not reveal a key on failure', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() =>
        Promise.resolve({
          ok: false,
          status: 403,
          statusText: 'Forbidden',
          headers: new Headers(),
          text: () => Promise.resolve(JSON.stringify({ error: 'forbidden' })),
        } as unknown as Response),
      ),
    );

    render(
      <RotateKeyDialog
        projectId="p-1"
        testerId="t-1"
        testerName="eastus-1"
        onClose={() => {}}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /rotate key/i }));

    await waitFor(() =>
      expect(screen.getByText(/forbidden/i)).toBeInTheDocument(),
    );
    expect(screen.queryByDisplayValue(NEW_KEY)).not.toBeInTheDocument();
  });
});
