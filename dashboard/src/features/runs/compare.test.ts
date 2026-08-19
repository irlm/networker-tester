import { describe, expect, it } from 'vitest';
import type { BenchmarkArtifact, ComparisonCell, LiveAttempt, TestRun } from '../../api/types';
import {
  artifactCaseStats,
  buildCaseMatrix,
  buildCellResult,
  cellMetaFromEndpoint,
  computeCellStats,
  envLabel,
  groupByEnvironment,
  groupByLanguage,
  parseCellLabel,
  resolveCellMeta,
  stripCellNameSuffix,
} from './compare';

// ── Fixtures ──────────────────────────────────────────────────────────────────

let runSeq = 0;

function run(configName: string, overrides: Partial<TestRun> = {}): TestRun {
  runSeq += 1;
  return {
    id: `run-${runSeq}`,
    test_config_id: `cfg-${runSeq}`,
    project_id: 'p-1',
    status: 'completed',
    started_at: '2026-08-18T12:00:00Z',
    finished_at: '2026-08-18T12:05:00Z',
    success_count: 0,
    failure_count: 0,
    error_message: null,
    artifact_id: null,
    tester_id: null,
    worker_id: null,
    last_heartbeat: null,
    created_at: '2026-08-18T12:00:00Z',
    config_name: configName,
    ...overrides,
  };
}

function httpAttempt(totalMs: number, success = true, ttfbMs = totalMs / 2): LiveAttempt {
  return {
    attempt_id: `a-${Math.random()}`,
    run_id: 'r',
    protocol: 'http1',
    sequence_num: 0,
    started_at: '2026-08-18T12:00:00Z',
    finished_at: '2026-08-18T12:00:01Z',
    success,
    retry_count: 0,
    http: success
      ? { status_code: 200, ttfb_ms: ttfbMs, total_duration_ms: totalMs, negotiated_version: 'HTTP/1.1' }
      : undefined,
  };
}

function attemptsAround(p50: number): LiveAttempt[] {
  // Symmetric around p50 so the interpolated median IS p50.
  return [httpAttempt(p50 - 10), httpAttempt(p50), httpAttempt(p50), httpAttempt(p50 + 10)];
}

function cell(configName: string, attempts: LiveAttempt[], groupCells?: ComparisonCell[]) {
  return buildCellResult(run(configName), attempts, null, groupCells);
}

// ── Name / label parsing ──────────────────────────────────────────────────────

describe('stripCellNameSuffix', () => {
  it('strips the launch suffix the control plane appends', () => {
    expect(stripCellNameSuffix('go @ azure/eastus @ linux @ nginx · cg-485406f0·0·ab12'))
      .toBe('go @ azure/eastus @ linux @ nginx');
  });

  it('strips it from full-stack labels that themselves contain the separator', () => {
    expect(stripCellNameSuffix('azure/eastus linux · Caddy · cg-deadbeef·3·9f2c'))
      .toBe('azure/eastus linux · Caddy');
  });

  it('leaves names without the suffix unchanged', () => {
    expect(stripCellNameSuffix('Checkout connectivity')).toBe('Checkout connectivity');
    expect(stripCellNameSuffix('azure/eastus linux · nginx')).toBe('azure/eastus linux · nginx');
  });
});

describe('parseCellLabel — application benchmark shape', () => {
  it('parses the four-part label', () => {
    expect(parseCellLabel('go @ azure/eastus @ linux @ nginx')).toEqual({
      language: 'go',
      cloud: 'azure',
      region: 'eastus',
      os: 'linux',
      proxy: 'nginx',
    });
  });

  it('parses the language-less label', () => {
    expect(parseCellLabel('azure/eastus @ windows @ iis')).toEqual({
      language: null,
      cloud: 'azure',
      region: 'eastus',
      os: 'windows',
      proxy: 'iis',
    });
  });

  it('does not confuse the nginx baseline language with the proxy axis', () => {
    expect(parseCellLabel('nginx @ azure/eastus @ linux @ caddy')).toEqual({
      language: 'nginx',
      cloud: 'azure',
      region: 'eastus',
      os: 'linux',
      proxy: 'caddy',
    });
  });
});

describe('parseCellLabel — full-stack shape', () => {
  it('parses cloud/region, os, and proxy label', () => {
    expect(parseCellLabel('azure/eastus linux · Caddy')).toEqual({
      language: null,
      cloud: 'azure',
      region: 'eastus',
      os: 'linux',
      proxy: 'caddy',
    });
  });

  it('parses Windows/IIS labels', () => {
    expect(parseCellLabel('aws/us-east-1 windows · IIS')).toEqual({
      language: null,
      cloud: 'aws',
      region: 'us-east-1',
      os: 'windows',
      proxy: 'iis',
    });
  });
});

describe('parseCellLabel — defensive fallbacks', () => {
  it('returns all-null axes for a custom name', () => {
    expect(parseCellLabel('My custom cell')).toEqual({
      language: null, cloud: null, region: null, os: null, proxy: null,
    });
  });

  it('does not invent a proxy from a stray separator in a custom name', () => {
    expect(parseCellLabel('Foo · Bar').proxy).toBeNull();
  });

  it('handles the empty string', () => {
    expect(parseCellLabel('')).toEqual({
      language: null, cloud: null, region: null, os: null, proxy: null,
    });
  });
});

describe('structured cell metadata', () => {
  const groupCells: ComparisonCell[] = [
    {
      label: 'go @ azure/eastus @ linux @ nginx',
      endpoint: {
        kind: 'pending',
        cloud_account_id: 'acc-1',
        region: 'eastus2',
        vm_size: 'Standard_B2s',
        os: 'windows',
        proxy_stack: 'iis',
        topology: 'loopback',
      },
    },
  ];

  it('extracts axes from a pending endpoint ref', () => {
    expect(cellMetaFromEndpoint(groupCells[0].endpoint)).toEqual({
      region: 'eastus2',
      os: 'windows',
      proxy: 'iis',
    });
  });

  it('lets structured fields win over name parsing', () => {
    const meta = resolveCellMeta('go @ azure/eastus @ linux @ nginx', groupCells);
    expect(meta).toEqual({
      language: 'go',
      cloud: 'azure',   // label only — pending refs carry no cloud name
      region: 'eastus2', // structured wins
      os: 'windows',
      proxy: 'iis',
    });
  });

  it('ignores network endpoint refs (rewritten cells)', () => {
    expect(cellMetaFromEndpoint({ kind: 'network', host: 'example.com' })).toEqual({});
  });
});

// ── Attempt-derived stats ─────────────────────────────────────────────────────

describe('computeCellStats', () => {
  it('computes interpolated p50/p95 and success rate', () => {
    const attempts = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100].map((v) => httpAttempt(v, true, v / 2));
    const stats = computeCellStats(attempts);
    expect(stats.total?.p50).toBe(55); // (50+60)/2, linear interpolation
    expect(stats.total?.p95).toBeCloseTo(95.5, 5); // rank 8.55 → 90 + 0.55·10
    expect(stats.ttfb?.p50).toBe(27.5);
    expect(stats.successRate).toBe(100);
    expect(stats.samples).toBe(10);
  });

  it('counts failures in the success rate but not in the latency stats', () => {
    const stats = computeCellStats([httpAttempt(100), httpAttempt(100), httpAttempt(0, false)]);
    expect(stats.successRate).toBeCloseTo((2 / 3) * 100, 5);
    expect(stats.samples).toBe(2);
    expect(stats.total?.p50).toBe(100);
    expect(stats.attempts).toBe(3);
  });

  it('returns nulls for an empty attempt list', () => {
    const stats = computeCellStats([]);
    expect(stats.total).toBeNull();
    expect(stats.ttfb).toBeNull();
    expect(stats.successRate).toBeNull();
    expect(stats.samples).toBe(0);
  });

  it('skips successful attempts without HTTP timing (non-HTTP modes)', () => {
    const udpOnly: LiveAttempt = { ...httpAttempt(0), http: undefined };
    const stats = computeCellStats([udpOnly, httpAttempt(42)]);
    expect(stats.samples).toBe(1);
    expect(stats.total?.p50).toBe(42);
    expect(stats.successRate).toBe(100);
  });
});

// ── Pivot: by environment ─────────────────────────────────────────────────────

describe('groupByEnvironment', () => {
  it('groups cells sharing (cloud/region/os/proxy) and ranks by p50', () => {
    const cells = [
      cell('python @ azure/eastus @ linux @ nginx · cg-11111111·2·aaaa', attemptsAround(200)),
      cell('go @ azure/eastus @ linux @ nginx · cg-11111111·0·aaaa', attemptsAround(100)),
      cell('go @ azure/eastus @ linux @ caddy · cg-11111111·1·aaaa', attemptsAround(120)),
    ];
    const sections = groupByEnvironment(cells);
    expect(sections).toHaveLength(2);

    const nginx = sections.find((s) => s.label === 'azure/eastus · linux · nginx');
    expect(nginx).toBeDefined();
    expect(nginx!.cells.map((c) => c.meta.language)).toEqual(['go', 'python']); // fastest first
    const caddy = sections.find((s) => s.label === 'azure/eastus · linux · caddy');
    expect(caddy!.cells).toHaveLength(1);
  });

  it('puts cells without latency data last', () => {
    const cells = [
      cell('go @ azure/eastus @ linux @ nginx', []),
      cell('python @ azure/eastus @ linux @ nginx', attemptsAround(300)),
    ];
    const [section] = groupByEnvironment(cells);
    expect(section.cells.map((c) => c.meta.language)).toEqual(['python', 'go']);
  });

  it('labels unparsable cells as an unlabeled environment', () => {
    const [section] = groupByEnvironment([cell('My custom cell', attemptsAround(50))]);
    expect(section.label).toBe('unlabeled environment');
    expect(envLabel(section.cells[0].meta)).toBe('unlabeled environment');
  });
});

// ── Pivot: by language ────────────────────────────────────────────────────────

describe('groupByLanguage', () => {
  it('detects a single differing axis and computes deltas vs the fastest cell', () => {
    const cells = [
      cell('go @ azure/eastus @ linux @ caddy · cg-11111111·1·aaaa', attemptsAround(120)),
      cell('go @ azure/eastus @ linux @ nginx · cg-11111111·0·aaaa', attemptsAround(100)),
    ];
    const [section] = groupByLanguage(cells);
    expect(section.language).toBe('go');
    expect(section.differingAxes).toEqual(['proxy']);
    expect(section.multiVariable).toBe(false);
    expect(section.rows.map((r) => r.variantLabel)).toEqual(['nginx', 'caddy']);
    expect(section.rows[0].deltaPct).toBe(0);
    expect(section.rows[1].deltaPct).toBeCloseTo(20, 5); // 120 vs 100
  });

  it('flags groups whose cells differ in more than one axis', () => {
    const cells = [
      cell('go @ azure/eastus @ linux @ nginx', attemptsAround(100)),
      cell('go @ azure/eastus @ windows @ iis', attemptsAround(180)),
    ];
    const [section] = groupByLanguage(cells);
    expect(section.differingAxes).toEqual(['os', 'proxy']);
    expect(section.multiVariable).toBe(true);
    expect(section.rows.map((r) => r.variantLabel)).toEqual(['linux · nginx', 'windows · iis']);
    expect(section.rows[1].deltaPct).toBeCloseTo(80, 5);
  });

  it('uses the full environment label when nothing differs (single cell)', () => {
    const [section] = groupByLanguage([
      cell('go @ azure/eastus @ linux @ nginx', attemptsAround(100)),
    ]);
    expect(section.differingAxes).toEqual([]);
    expect(section.rows[0].variantLabel).toBe('azure/eastus · linux · nginx');
    expect(section.rows[0].deltaPct).toBe(0);
  });

  it('sorts named languages alphabetically and the unlabeled bucket last', () => {
    const sections = groupByLanguage([
      cell('azure/eastus linux · Caddy', attemptsAround(90)), // no language (full-stack)
      cell('python @ azure/eastus @ linux @ nginx', attemptsAround(200)),
      cell('go @ azure/eastus @ linux @ nginx', attemptsAround(100)),
    ]);
    expect(sections.map((s) => s.language)).toEqual(['go', 'python', '']);
  });

  it('leaves deltas null when the fastest cell has no latency data', () => {
    const [section] = groupByLanguage([
      cell('go @ azure/eastus @ linux @ nginx', []),
      cell('go @ azure/eastus @ linux @ caddy', []),
    ]);
    expect(section.rows.every((r) => r.deltaPct === null)).toBe(true);
  });
});

// ── Artifact per-case depth (post-#796) ───────────────────────────────────────

function artifactWith(cases: BenchmarkArtifact['cases'], summaries: unknown[]): BenchmarkArtifact {
  return { cases, summaries } as unknown as BenchmarkArtifact;
}

const sortCase = { id: 'sort', protocol: 'http1', payload_bytes: null, http_stack: null, metric_name: 'latency', metric_unit: 'ms', higher_is_better: false };
const hashCase = { ...sortCase, id: 'hash' };

function summaryFor(caseId: string, p50: number, p95: number, success = 15, failure = 0) {
  return {
    case_id: caseId, protocol: 'http1', payload_bytes: null, http_stack: null,
    metric_name: 'latency', metric_unit: 'ms', higher_is_better: false,
    sample_count: success + failure, included_sample_count: success,
    success_count: success, failure_count: failure,
    p50, p95,
  };
}

describe('artifactCaseStats', () => {
  it('returns [] when cases is empty (the pre-#796 gap)', () => {
    expect(artifactCaseStats(artifactWith([], [summaryFor('sort', 1, 2)]))).toEqual([]);
    expect(artifactCaseStats(null)).toEqual([]);
  });

  it('joins summaries with cases and computes the per-case success rate', () => {
    const stats = artifactCaseStats(
      artifactWith([sortCase, hashCase], [summaryFor('sort', 3.2, 7.9), summaryFor('hash', 1.1, 2.0, 12, 3)]),
    );
    expect(stats).toHaveLength(2);
    expect(stats[0]).toMatchObject({ caseId: 'sort', label: 'sort', p50: 3.2, p95: 7.9, successRate: 100 });
    expect(stats[1].successRate).toBe(80);
    expect(stats[1].samples).toBe(12);
  });

  it('ignores summaries whose case_id has no case row', () => {
    const stats = artifactCaseStats(artifactWith([sortCase], [summaryFor('unknown', 1, 2)]));
    expect(stats).toEqual([]);
  });
});

describe('buildCaseMatrix', () => {
  it('collects the union of case labels and skips cells without per-case data', () => {
    const withCases = buildCellResult(
      run('go @ azure/eastus @ linux @ nginx'),
      attemptsAround(100),
      artifactWith([sortCase, hashCase], [summaryFor('sort', 3, 6), summaryFor('hash', 1, 2)]),
    );
    const withoutCases = buildCellResult(
      run('python @ azure/eastus @ linux @ nginx'),
      attemptsAround(200),
      artifactWith([], []),
    );
    const matrix = buildCaseMatrix([withCases, withoutCases]);
    expect(matrix.caseLabels).toEqual(['sort', 'hash']);
    expect(matrix.columns).toHaveLength(1);
    expect(matrix.columns[0].cell).toBe(withCases);
    expect(matrix.columns[0].byCase.get('sort')?.p50).toBe(3);
  });
});
