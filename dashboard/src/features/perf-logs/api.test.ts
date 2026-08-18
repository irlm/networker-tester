import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useApiLogStore } from '../../stores/apiLogStore';
import { perfLogsApi } from './api';

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  });
}

const fetchMock = vi.fn();

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-08-18T12:00:00.000Z'));
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockReset();
  fetchMock.mockResolvedValue(jsonResponse([]));
  useApiLogStore.setState({ enabled: false });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function requestedUrl(): URL {
  return new URL(String(fetchMock.mock.calls[0]?.[0]), 'https://dashboard.test');
}

describe('performance log API', () => {
  it('turns the default five-minute window into a server-side since filter', async () => {
    const signal = new AbortController().signal;

    await perfLogsApi.list({ kind: 'api', path: '/v2/projects', windowMs: 300_000, limit: 200 }, signal);

    const url = requestedUrl();
    expect(url.pathname).toBe('/api/perf-log');
    expect(url.searchParams.get('since')).toBe('2026-08-18T11:55:00.000Z');
    expect(url.searchParams.get('kind')).toBe('api');
    expect(url.searchParams.get('path')).toBe('/v2/projects');
    expect(url.searchParams.get('limit')).toBe('200');
    expect(fetchMock).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ signal }),
    );
  });

  it('uses the same selected window for aggregate statistics', async () => {
    fetchMock.mockResolvedValueOnce(jsonResponse({ api_count: 0, render_count: 0 }));

    await perfLogsApi.stats(60 * 60_000);

    expect(requestedUrl().pathname).toBe('/api/perf-log/stats');
    expect(requestedUrl().searchParams.get('since')).toBe('2026-08-18T11:00:00.000Z');
  });

  it('omits optional filters when they are not requested', async () => {
    await perfLogsApi.list();

    expect(requestedUrl().search).toBe('');
  });
});
