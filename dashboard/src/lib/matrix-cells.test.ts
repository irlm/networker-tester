import { describe, expect, it } from 'vitest';
import { buildComparisonCells, countCells, isMatrixRun } from './matrix-cells';
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
