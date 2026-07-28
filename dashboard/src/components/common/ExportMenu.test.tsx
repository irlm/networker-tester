// ExportMenu behavior: the dropdown lists every export format, a selection
// fetches the report with the Bearer token and the right ?format=, the blob
// download is triggered with the server's Content-Disposition filename, and
// a failed export surfaces a toast instead of a silent no-op.

import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ExportMenu } from './ExportMenu';
import { useToastStore } from '../../hooks/useToast';

function mockFetchOk(disposition?: string): ReturnType<typeof vi.fn> {
  const fetchMock = vi.fn().mockResolvedValue({
    ok: true,
    status: 200,
    headers: new Headers(disposition ? { 'Content-Disposition': disposition } : {}),
    blob: () => Promise.resolve(new Blob(['%PDF-fake'])),
  });
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('ExportMenu', () => {
  beforeEach(() => {
    localStorage.setItem('token', 'test-token');
    // jsdom has no URL.createObjectURL — stub the object-URL lifecycle.
    URL.createObjectURL = vi.fn(() => 'blob:fake');
    URL.revokeObjectURL = vi.fn();
    useToastStore.setState({ toasts: [] });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it('lists all four document formats', async () => {
    mockFetchOk();
    render(<ExportMenu path="/projects/p1/reports/integrated" fileBase="integrated-report" />);

    await userEvent.click(screen.getByRole('button', { name: /export/i }));

    for (const label of ['PDF', 'HTML', 'Word (.docx)', 'Markdown']) {
      expect(screen.getByRole('menuitem', { name: label })).toBeInTheDocument();
    }
  });

  it('downloads with the Bearer token, the chosen format, and the server filename', async () => {
    const fetchMock = mockFetchOk('attachment; filename="integrated-report-p1.pdf"');
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    render(<ExportMenu path="/projects/p1/reports/integrated" fileBase="integrated-report" />);

    await userEvent.click(screen.getByRole('button', { name: /export/i }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'PDF' }));

    await waitFor(() => expect(clickSpy).toHaveBeenCalled());
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/projects/p1/reports/integrated?format=pdf',
      expect.objectContaining({
        headers: expect.objectContaining({ Authorization: 'Bearer test-token' }),
      }),
    );
    // The anchor that was clicked carries the server-supplied filename.
    const anchor = clickSpy.mock.instances[0] as HTMLAnchorElement;
    expect(anchor.download).toBe('integrated-report-p1.pdf');
  });

  it('surfaces a toast when the export fails', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: false,
      status: 500,
      statusText: 'Internal Server Error',
      headers: new Headers(),
      text: () => Promise.resolve('boom'),
    }));
    render(<ExportMenu path="/projects/p1/reports/integrated" fileBase="integrated-report" />);

    await userEvent.click(screen.getByRole('button', { name: /export/i }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'HTML' }));

    await waitFor(() => {
      expect(useToastStore.getState().toasts.some(t =>
        t.type === 'error' && t.message.includes('Export failed'))).toBe(true);
    });
  });
});
