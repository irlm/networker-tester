import { afterEach, describe, expect, it, vi } from 'vitest';
import type { BenchmarkArtifact, TestRun } from '../../api/types';
import type { CellResult } from './compare';
import { encodeRun, scorecardBaseUrl, scorecardHref, scorecardRun } from './scorecardLink';

function artifact(rps: number[], cores = 4, scenario = 'warm'): BenchmarkArtifact {
  return {
    environment: { server_info: { os: 'linux', arch: 'x86_64', cpu_cores: cores } },
    methodology: { scenario },
    summaries: rps.map((r, i) => ({ case_id: `c${String(i)}`, rps: r, latency_p50_ms: 2 + i, latency_p99_ms: 9 + i })),
  } as unknown as BenchmarkArtifact;
}

function cell(language: string | null, art: BenchmarkArtifact | null): CellResult {
  return {
    run: { id: `run-${language ?? 'x'}`, started_at: '2026-09-20T10:00:00Z' } as TestRun,
    cellLabel: language ?? 'cell',
    meta: { language, cloud: 'azure', region: 'eastus', os: 'linux', proxy: null },
    stats: { attempts: 0, samples: 0, successRate: null, ttfb: null, total: null },
    artifact: art,
  };
}

function decode(href: string): unknown {
  const v = href.split('#laghound=')[1] ?? '';
  const b64 = v.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (v.length % 4)) % 4);
  return JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)))) as unknown;
}

describe('scorecard link', () => {
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it('builds one result per language from its busiest case, with cores and scenario', () => {
    const run = scorecardRun([cell('csharp-net48', artifact([900, 1200])), cell('go', artifact([5000], 4)), cell('python', null), cell(null, artifact([10]))], 'grp eastus');
    expect(run?.results).toEqual([
      { language: 'csharp-net48', scenario: 'warm', environment: { server_cpu_cores: 4 }, network: { rps: 1200, latency_p50_ms: 3, latency_p99_ms: 10 } },
      { language: 'go', scenario: 'warm', environment: { server_cpu_cores: 4 }, network: { rps: 5000, latency_p50_ms: 2, latency_p99_ms: 9 } },
    ]);
    expect(run?.started_at).toBe('2026-09-20T10:00:00Z');
  });

  it('needs two languages with results to compare', () => {
    expect(scorecardRun([cell('go', artifact([5000])), cell('python', null)], 'x')).toBeUndefined();
  });

  it('carries the run in the fragment, base64url, readable back as UTF-8 JSON', () => {
    const run = { id: 'grupo café', results: [{ language: 'go', environment: {}, network: { rps: 1 } }] };
    const href = scorecardHref('https://scorecard.example/app#old', run);
    expect(href.startsWith('https://scorecard.example/app#laghound=')).toBe(true);
    expect(encodeRun(run)).not.toMatch(/[+/=]/);
    expect(decode(href)).toEqual(run);
  });

  it('is only offered when VITE_INFRA_SCORECARD_URL is an http(s) URL', () => {
    vi.stubEnv('VITE_INFRA_SCORECARD_URL', '');
    expect(scorecardBaseUrl()).toBeUndefined();
    vi.stubEnv('VITE_INFRA_SCORECARD_URL', 'javascript:alert(1)');
    expect(scorecardBaseUrl()).toBeUndefined();
    vi.stubEnv('VITE_INFRA_SCORECARD_URL', 'https://scorecard.example/');
    expect(scorecardBaseUrl()).toBe('https://scorecard.example');
  });

  it('accepts a path on this site (the release bundles the scorecard at /scorecard/), never a protocol-relative one', () => {
    vi.stubEnv('VITE_INFRA_SCORECARD_URL', '/scorecard/');
    expect(scorecardBaseUrl()).toBe('/scorecard/');
    vi.stubEnv('VITE_INFRA_SCORECARD_URL', '/scorecard');
    expect(scorecardBaseUrl()).toBe('/scorecard/');
    vi.stubEnv('VITE_INFRA_SCORECARD_URL', '//evil.example/x');
    expect(scorecardBaseUrl()).toBeUndefined();
    const run = { id: 'g', results: [{ language: 'go', environment: {}, network: { rps: 1 } }] };
    expect(scorecardHref('/scorecard/', run)).toMatch(/^\/scorecard\/#laghound=/);
  });
});
