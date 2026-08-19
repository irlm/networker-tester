import { describe, expect, it } from 'vitest';
import type { TestRun } from '../../api/types';
import {
  clusterComparisonGroups,
  groupAggregateStatus,
  groupFallbackName,
  groupProgress,
  sortCellsByName,
} from './list-grouping';

function run(id: string, overrides: Partial<TestRun> = {}): TestRun {
  return {
    id,
    test_config_id: `cfg-${id}`,
    project_id: 'p-1',
    status: 'completed',
    started_at: '2026-08-18T12:00:00Z',
    finished_at: '2026-08-18T12:05:00Z',
    success_count: 4,
    failure_count: 0,
    error_message: null,
    artifact_id: null,
    tester_id: null,
    worker_id: null,
    last_heartbeat: null,
    created_at: '2026-08-18T12:00:00Z',
    ...overrides,
  };
}

const G1 = 'aaaaaaaa-0000-4000-8000-000000000001';
const G2 = 'bbbbbbbb-0000-4000-8000-000000000002';

describe('clusterComparisonGroups', () => {
  it('collapses runs sharing a comparison_group_id into one group row at the newest member position', () => {
    const rows = clusterComparisonGroups([
      run('r1', { comparison_group_id: G1 }),
      run('standalone-1'),
      run('r2', { comparison_group_id: G1 }),
      run('r3', { comparison_group_id: G1 }),
    ]);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ kind: 'group', groupId: G1 });
    expect((rows[0] as { runs: TestRun[] }).runs.map((r) => r.id)).toEqual(['r1', 'r2', 'r3']);
    expect(rows[1]).toMatchObject({ kind: 'run', run: { id: 'standalone-1' } });
  });

  it('keeps a mixed page of standalone runs and several groups in list order', () => {
    const rows = clusterComparisonGroups([
      run('s1'),
      run('a1', { comparison_group_id: G1 }),
      run('b1', { comparison_group_id: G2 }),
      run('a2', { comparison_group_id: G1 }),
      run('s2', { comparison_group_id: null }),
      run('b2', { comparison_group_id: G2 }),
    ]);
    expect(rows.map((r) => (r.kind === 'group' ? `group:${r.groupId}` : r.run.id))).toEqual([
      's1',
      `group:${G1}`,
      `group:${G2}`,
      's2',
    ]);
  });

  it('a single-cell group still clusters (one row, one member)', () => {
    const rows = clusterComparisonGroups([run('only', { comparison_group_id: G1 })]);
    expect(rows).toEqual([{ kind: 'group', groupId: G1, runs: [expect.objectContaining({ id: 'only' })] }]);
  });
});

describe('groupProgress', () => {
  const cells = [
    run('done'),
    run('bad', { status: 'failed' }),
    run('all-failed', { status: 'completed', success_count: 0, failure_count: 5 }),
    run('active', { status: 'running' }),
  ];

  it('uses the group API cell count when available', () => {
    const p = groupProgress(cells, 14);
    // Terminal: completed + failed + completed-all-failed. Failed verdicts:
    // status failed + completed where everything failed (runDisplayStatus).
    expect(p).toEqual({ terminal: 3, failed: 2, total: 14, approximate: false });
  });

  it('falls back to the in-window count flagged approximate when the group is deleted (SET NULL)', () => {
    const p = groupProgress(cells, null);
    expect(p.total).toBe(4);
    expect(p.approximate).toBe(true);
  });

  it('never reports fewer total cells than are visible (relaunched groups)', () => {
    expect(groupProgress(cells, 2).total).toBe(4);
  });
});

describe('groupAggregateStatus', () => {
  it('any active cell wins: running', () => {
    expect(groupAggregateStatus([run('a'), run('b', { status: 'queued' })])).toBe('running');
    expect(groupAggregateStatus([run('a', { status: 'failed' }), run('b', { status: 'provisioning' })])).toBe('running');
  });

  it('all terminal without failures: completed', () => {
    expect(groupAggregateStatus([run('a'), run('b')])).toBe('completed');
  });

  it('all terminal, mixed verdicts: partial (failed-mixed)', () => {
    expect(groupAggregateStatus([run('a'), run('b', { status: 'failed' })])).toBe('partial');
    // completed-but-everything-failed counts as a failed verdict, not success.
    expect(
      groupAggregateStatus([run('a'), run('b', { success_count: 0, failure_count: 3 })]),
    ).toBe('partial');
  });

  it('every cell failed: failed; every cell cancelled: cancelled', () => {
    expect(groupAggregateStatus([run('a', { status: 'failed' }), run('b', { status: 'failed' })])).toBe('failed');
    expect(groupAggregateStatus([run('a', { status: 'cancelled' }), run('b', { status: 'cancelled' })])).toBe('cancelled');
  });

  it('an empty window is queued, not completed', () => {
    expect(groupAggregateStatus([])).toBe('queued');
  });
});

describe('groupFallbackName', () => {
  it('uses the shared config-name prefix with the cg suffix stripped', () => {
    expect(
      groupFallbackName([
        run('a', { config_name: 'azure/eastus linux · nginx · cg-aaaaaaaa·0·ab12' }),
        run('b', { config_name: 'azure/eastus linux · caddy · cg-aaaaaaaa·1·ab12' }),
      ]),
    ).toBe('azure/eastus linux');
  });

  it('returns null when the cells share no meaningful prefix (deleted-group fallback → Group <id>)', () => {
    expect(
      groupFallbackName([
        run('a', { config_name: 'go @ azure/eastus @ linux @ nginx' }),
        run('b', { config_name: 'python @ azure/eastus @ linux @ nginx' }),
      ]),
    ).toBeNull();
    expect(groupFallbackName([run('a', { config_name: undefined })])).toBeNull();
  });
});

describe('sortCellsByName', () => {
  it('orders by suffix-stripped config name with id tiebreak, without mutating input', () => {
    const input = [
      run('r-z', { config_name: 'python @ az · cg-aaaaaaaa·1·ab12' }),
      run('r-b', { config_name: 'go @ az · cg-aaaaaaaa·0·ab12' }),
      run('r-a', { config_name: 'go @ az · cg-aaaaaaaa·0·ffff' }),
    ];
    const sorted = sortCellsByName(input);
    expect(sorted.map((r) => r.id)).toEqual(['r-a', 'r-b', 'r-z']);
    expect(input.map((r) => r.id)).toEqual(['r-z', 'r-b', 'r-a']);
  });
});
