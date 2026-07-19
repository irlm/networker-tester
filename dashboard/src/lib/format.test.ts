import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { timeAgo, formatDuration, formatMsCompact } from './format';

const NOW = new Date('2026-07-17T12:00:00Z');

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  vi.useRealTimers();
});

describe('timeAgo', () => {
  it('renders seconds under a minute', () => {
    expect(timeAgo('2026-07-17T11:59:18Z')).toBe('42s ago');
  });

  it('renders minutes under an hour', () => {
    expect(timeAgo('2026-07-17T11:35:00Z')).toBe('25m ago');
  });

  it('renders hours under a day', () => {
    expect(timeAgo('2026-07-17T07:00:00Z')).toBe('5h ago');
  });

  it('renders days beyond 24h', () => {
    expect(timeAgo('2026-07-14T12:00:00Z')).toBe('3d ago');
  });

  it('clamps future timestamps to "just now"', () => {
    expect(timeAgo('2026-07-17T12:00:30Z')).toBe('just now');
  });

  it('falls back to the raw string on invalid input', () => {
    expect(timeAgo('not-a-date')).toBe('not-a-date');
  });
});

describe('formatDuration', () => {
  it('returns an em dash without a start', () => {
    expect(formatDuration(null, null)).toBe('—');
  });

  it('formats sub-second durations in ms', () => {
    expect(formatDuration('2026-07-17T11:59:59.500Z', '2026-07-17T11:59:59.900Z')).toBe('400ms');
  });

  it('formats seconds under a minute', () => {
    expect(formatDuration('2026-07-17T11:59:15Z', '2026-07-17T11:59:57Z')).toBe('42s');
  });

  it('formats minutes with remainder seconds', () => {
    expect(formatDuration('2026-07-17T11:55:10Z', '2026-07-17T11:58:40Z')).toBe('3m 30s');
  });

  it('formats hours with remainder minutes', () => {
    expect(formatDuration('2026-07-17T09:30:00Z', '2026-07-17T11:45:00Z')).toBe('2h 15m');
  });

  it('uses now as the end when end is missing', () => {
    expect(formatDuration('2026-07-17T11:59:20Z', null)).toBe('40s');
  });
});

describe('formatMsCompact', () => {
  it('renders "-" for null and undefined', () => {
    expect(formatMsCompact(null)).toBe('-');
    expect(formatMsCompact(undefined)).toBe('-');
  });

  it('renders "<1ms" for sub-millisecond values', () => {
    expect(formatMsCompact(0.4)).toBe('<1ms');
  });

  it('renders one decimal place otherwise', () => {
    expect(formatMsCompact(12.34)).toBe('12.3ms');
    expect(formatMsCompact(250)).toBe('250.0ms');
  });
});
