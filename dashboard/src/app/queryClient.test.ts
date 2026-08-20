import { describe, expect, it } from 'vitest';
import { ApiError } from '../api/http';
import { queryClient } from './queryClient';

describe('shared query client policy', () => {
  it('uses the dashboard freshness and focus-refetch defaults', () => {
    const options = queryClient.getDefaultOptions();

    expect(options.queries?.staleTime).toBe(10_000);
    expect(options.queries?.gcTime).toBe(5 * 60_000);
    // Focus refetch is ON (freshness audit): returning to the tab refreshes
    // whatever is on screen; staleTime bounds the burst.
    expect(options.queries?.refetchOnWindowFocus).toBe(true);
    expect(options.mutations?.retry).toBe(false);
  });

  it('does not retry client errors', () => {
    const retry = queryClient.getDefaultOptions().queries?.retry;
    expect(typeof retry).toBe('function');
    if (typeof retry !== 'function') throw new Error('Expected functional retry policy');

    expect(retry(0, new ApiError(404, 'Not found'))).toBe(false);
    expect(retry(0, new ApiError(429, 'Rate limited'))).toBe(false);
  });

  it('bounds retries for transient and network failures', () => {
    const retry = queryClient.getDefaultOptions().queries?.retry;
    expect(typeof retry).toBe('function');
    if (typeof retry !== 'function') throw new Error('Expected functional retry policy');

    expect(retry(0, new ApiError(503, 'Unavailable'))).toBe(true);
    expect(retry(1, new TypeError('Network failed'))).toBe(true);
    expect(retry(2, new TypeError('Network failed'))).toBe(false);
  });
});
