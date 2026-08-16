import { describe, expect, it } from 'vitest';
import { buildComparisonCells, countCells, isMatrixRun, h3DropsPerCell } from './matrix-cells';
import { makeTestbed } from '../components/wizard/testbed-constants';

/**
 * Audit P1-12: FullStackPage — the matrix wizard, and the feature whose
 * end-to-end path was broken for the entire v0.28.129-147 campaign — had NO
 * test file. These pin the function that decides how many cells a launch
 * creates, how each is labelled, and which runner they pin to.
 */
describe('buildComparisonCells', () => {
  const tb = (over: Partial<ReturnType<typeof makeTestbed>> = {}) => ({
    ...makeTestbed(1, 'Azure', 'linux', ['nginx']),
    cloudAccountId: 'acct-1',
    region: 'eastus',
    ...over,
  });

  it('produces one cell per (testbed × proxy)', () => {
    const cells = buildComparisonCells(
      [tb({ proxies: ['nginx', 'caddy', 'traefik'] })],
      null,
    );
    expect(cells).toHaveLength(3);
    const stacks = cells.map((c) => (c.endpoint.kind === 'pending' ? c.endpoint.proxy_stack : undefined));
    expect(stacks.sort()).toEqual(['caddy', 'nginx', 'traefik']);
  });

  it('fans out across multiple testbeds', () => {
    const cells = buildComparisonCells(
      [
        tb({ proxies: ['nginx', 'caddy'] }),
        tb({ region: 'westus', proxies: ['haproxy'] }),
      ],
      null,
    );
    expect(cells).toHaveLength(3);
    expect(cells.filter((c) => c.endpoint.kind === 'pending' && c.endpoint.region === 'westus')).toHaveLength(1);
  });

  it('gives every cell of a matrix a DISTINCT label', () => {
    // The v0.28.129 regression class: cells sharing a name prefix collided on
    // the derived VM name and on UNIQUE(project_id, name). Distinct labels are
    // the first line of defence.
    const cells = buildComparisonCells(
      [
        tb({ proxies: ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'] }),
        tb({ region: 'westus', proxies: ['nginx', 'caddy'] }),
      ],
      null,
    );
    const labels = cells.map((c) => c.label);
    expect(new Set(labels).size).toBe(labels.length);
  });

  it('marks every cell as a pending endpoint so the orchestrator provisions it', () => {
    const cells = buildComparisonCells([tb({ proxies: ['nginx', 'caddy'] })], null);
    for (const c of cells) {
      // Narrow the EndpointRef union before reading pending-only fields —
      // if a cell were ever built as another kind, this assertion fails
      // rather than the property access silently type-erroring.
      const ep = c.endpoint;
      expect(ep.kind).toBe('pending');
      if (ep.kind !== 'pending') continue;
      expect(ep.cloud_account_id).toBe('acct-1');
      expect(ep.os).toBe('linux');
      expect(ep.vm_size).toBeTruthy();
    }
  });

  it('pins the runner only when one was explicitly selected', () => {
    const withRunner = buildComparisonCells([tb()], 'tester-123');
    expect(withRunner[0].runner_id).toBe('tester-123');

    const auto = buildComparisonCells([tb()], null);
    // Absent (not null/empty) so the server picks a runner itself.
    expect('runner_id' in auto[0]).toBe(false);
  });

  it('returns no cells when a testbed has no proxies selected', () => {
    expect(buildComparisonCells([tb({ proxies: [] })], null)).toHaveLength(0);
  });

  it('labels carry cloud, region, os and a human proxy name', () => {
    const [cell] = buildComparisonCells([tb({ proxies: ['haproxy'] })], null);
    expect(cell.label).toContain('Azure');
    expect(cell.label).toContain('eastus');
    expect(cell.label).toContain('linux');
    expect(cell.label).toContain('HAProxy'); // PROXY_LABELS display form
  });
});

describe('countCells / isMatrixRun', () => {
  const base = { ...makeTestbed(1, 'Azure', 'linux', ['nginx']), cloudAccountId: 'a', region: 'eastus' };

  it('a single testbed with one proxy is NOT a matrix', () => {
    expect(countCells([base])).toBe(1);
    expect(isMatrixRun([base])).toBe(false);
  });

  it('one testbed with several proxies IS a matrix', () => {
    const multi = [{ ...base, proxies: ['nginx', 'caddy'] }];
    expect(countCells(multi)).toBe(2);
    expect(isMatrixRun(multi)).toBe(true);
  });

  it('several testbeds with one proxy each IS a matrix', () => {
    expect(isMatrixRun([base, { ...base, region: 'westus' }])).toBe(true);
  });

  it('an empty testbed list is not a matrix', () => {
    expect(countCells([])).toBe(0);
    expect(isMatrixRun([])).toBe(false);
  });
});

describe('h3DropsPerCell (per-cell HTTP/3 trim preview)', () => {
  const base = { ...makeTestbed(1, 'Azure', 'linux', ['nginx']), cloudAccountId: 'a', region: 'eastus' };
  const modes = ['http1', 'http2', 'http3', 'pageload3', 'download'];

  it('a matrix over nginx + apache drops the h3 modes on the apache cell only', () => {
    const drops = h3DropsPerCell([{ ...base, proxies: ['nginx', 'apache'] }], modes);
    expect(drops).toHaveLength(1);
    expect(drops[0].stack).toBe('apache');
    expect(drops[0].label).toBe('Azure/eastus linux · Apache');
    expect(drops[0].dropped).toEqual(['http3', 'pageload3']);
  });

  it('nothing is dropped when every stack serves QUIC or no h3 mode is selected', () => {
    expect(h3DropsPerCell([{ ...base, proxies: ['nginx', 'caddy'] }], modes)).toEqual([]);
    expect(h3DropsPerCell([{ ...base, proxies: ['apache', 'haproxy'] }], ['http1', 'download'])).toEqual([]);
    expect(h3DropsPerCell([], modes)).toEqual([]);
  });

  it('lists one entry per quic-less cell across testbeds, in cell order', () => {
    const drops = h3DropsPerCell(
      [{ ...base, proxies: ['haproxy'] }, { ...base, region: 'westus', proxies: ['nginx', 'traefik'] }],
      ['http3'],
    );
    expect(drops.map(d => `${d.label}|${d.stack}`)).toEqual([
      'Azure/eastus linux · HAProxy|haproxy',
      'Azure/westus linux · Traefik|traefik',
    ]);
  });
});
