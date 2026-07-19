import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { api, ApiError, clearSession, errorMessage, friendlyHttpError } from './client';
import { useApiLogStore } from '../stores/apiLogStore';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const fetchMock = vi.fn();

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockReset();
  localStorage.setItem('token', 'test-token');
  // Silence the api-log side channel — it's exercised implicitly.
  useApiLogStore.setState({ enabled: false });
});

afterEach(() => {
  vi.unstubAllGlobals();
  clearSession();
});

describe('request()', () => {
  it('parses JSON bodies', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ providers: [] }));
    await expect(api.getProviders()).resolves.toEqual({ providers: [] });
    expect(fetchMock).toHaveBeenCalledWith(
      '/api/auth/sso/providers',
      expect.objectContaining({
        headers: expect.objectContaining({ Authorization: 'Bearer test-token' }),
      }),
    );
  });

  it('resolves undefined on empty-body responses (204 / NoContent)', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await expect(api.deleteTestConfig('cfg-1')).resolves.toBeUndefined();
  });

  it('does NOT wipe the session on 401 from /auth/login (bad credentials)', async () => {
    fetchMock.mockResolvedValueOnce(new Response('invalid credentials', { status: 401 }));
    await expect(api.login('a@b.c', 'wrong')).rejects.toMatchObject({
      name: 'ApiError',
      status: 401,
    });
    // Session survives — the login page must show the error, not reload.
    expect(localStorage.getItem('token')).toBe('test-token');
  });

  it('wipes the session on 401 from authenticated endpoints', async () => {
    fetchMock.mockResolvedValueOnce(new Response('', { status: 401 }));
    localStorage.setItem('activeProjectId', 'p1');
    await expect(api.getProjects()).rejects.toMatchObject({ status: 401 });
    expect(localStorage.getItem('token')).toBeNull();
    expect(localStorage.getItem('activeProjectId')).toBeNull();
  });

  it('throws a typed ApiError carrying status and body on 4xx/5xx', async () => {
    fetchMock.mockResolvedValueOnce(new Response('boom', { status: 500 }));
    const err = await api.getProjects().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(500);
    expect((err as ApiError).body).toBe('boom');
  });

  it('maps network-level failures to ApiError with status 0', async () => {
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    const err = await api.getProjects().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(0);
    expect((err as ApiError).message).toContain('Network error');
  });

  it('preserves AbortError so callers can distinguish cancellation', async () => {
    fetchMock.mockRejectedValueOnce(new DOMException('aborted', 'AbortError'));
    const err = await api.getProjects().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(DOMException);
    expect((err as DOMException).name).toBe('AbortError');
  });
});

describe('friendlyHttpError — human copy, never raw bodies (audit F3/F16)', () => {
  it('maps 404 to human copy without class-name or "API error:" prefixes', async () => {
    fetchMock.mockResolvedValueOnce(new Response('Not Found', { status: 404, statusText: 'Not Found' }));
    const err = await api.getTestRun('r1').catch((e: unknown) => e) as ApiError;
    expect(err.message).not.toContain('API error');
    expect(err.message).not.toContain('ApiError');
    expect(err.message).toContain('Not found');
    // errorMessage() must not re-introduce the class name like String(e) did.
    expect(errorMessage(err)).toBe(err.message);
  });

  it('never surfaces raw nginx 502 HTML in the message', async () => {
    const nginx = '<html>\n<head><title>502 Bad Gateway</title></head>\n<body><center><h1>502 Bad Gateway</h1></center></body>\n</html>\n<!-- a padding to disable MSIE and Chrome friendly error page -->';
    fetchMock.mockResolvedValueOnce(new Response(nginx, { status: 502, statusText: 'Bad Gateway' }));
    const err = await api.getTestRun('r1').catch((e: unknown) => e) as ApiError;
    expect(err.message).not.toContain('<');
    expect(err.message).toContain('Server unavailable');
    // Raw body stays available for debugging on .body.
    expect(err.body).toContain('502 Bad Gateway');
  });

  it('hides raw JSON internal-server-error bodies (5xx)', () => {
    expect(friendlyHttpError(500, 'Internal Server Error', '{"error":"internal server error"}'))
      .toBe('Server error — try again shortly.');
  });

  it('keeps useful 4xx server detail, capped', () => {
    expect(friendlyHttpError(409, 'Conflict', '{"error":"config name already exists"}'))
      .toContain('config name already exists');
    const long = 'x'.repeat(500);
    expect(friendlyHttpError(400, 'Bad Request', long).length).toBeLessThan(220);
  });
});

describe('getAgents response-shape normalization (P0 — black-screen regression)', () => {
  const agent = { agent_id: 'a1', name: 'runner-1', status: 'online' };

  it('accepts the legacy wrapped shape { agents: [...] }', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ agents: [agent] }));
    await expect(api.getAgents('p1')).resolves.toEqual([agent]);
  });

  it('accepts the C# control-plane bare-array shape', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse([agent]));
    await expect(api.getAgents('p1')).resolves.toEqual([agent]);
  });

  it('returns [] for empty/odd payloads instead of undefined', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({}));
    await expect(api.getAgents('p1')).resolves.toEqual([]);
  });
});

describe('checkEmail (dead endpoint stub)', () => {
  it('resolves locally without a network round-trip', async () => {
    await expect(api.checkEmail('user@example.com')).resolves.toEqual({ provider: null });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
