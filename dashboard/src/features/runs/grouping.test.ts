import { describe, expect, it } from 'vitest';
import { groupByProtocol, groupByTargetUrl } from './grouping';
import type { LiveAttempt } from '../../api/types';

function attempt(overrides: Partial<LiveAttempt>): LiveAttempt {
  return {
    attempt_id: Math.random().toString(36).slice(2),
    run_id: 'r1',
    protocol: 'http2',
    sequence_num: 0,
    started_at: '2026-08-18T00:00:00Z',
    finished_at: null,
    success: true,
    error_message: null,
    retry_count: 0,
    ...overrides,
  } as LiveAttempt;
}

describe('groupByTargetUrl (#782 URL sets)', () => {
  it('groups attempts per probed URL', () => {
    const groups = groupByTargetUrl([
      attempt({ target_url: 'https://a.example/' }),
      attempt({ target_url: 'https://b.example/' }),
      attempt({ target_url: 'https://a.example/' }),
    ]);
    expect(Object.keys(groups).sort()).toEqual(['https://a.example/', 'https://b.example/']);
    expect(groups['https://a.example/']).toHaveLength(2);
  });

  it("buckets attempts without target_url under '' (single-URL / pre-#782)", () => {
    const groups = groupByTargetUrl([attempt({}), attempt({ target_url: null })]);
    expect(Object.keys(groups)).toEqual(['']);
    expect(groups['']).toHaveLength(2);
  });

  it('composes with groupByProtocol inside each URL bucket', () => {
    const groups = groupByTargetUrl([
      attempt({ target_url: 'https://a.example/', protocol: 'dns' }),
      attempt({ target_url: 'https://a.example/', protocol: 'http2' }),
    ]);
    const protos = groupByProtocol(groups['https://a.example/']);
    expect(Object.keys(protos).sort()).toEqual(['dns', 'http2']);
  });
});
