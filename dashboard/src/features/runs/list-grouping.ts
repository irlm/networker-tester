/**
 * Group-first Runs list (#803): cluster the fetched run window so a
 * comparison-group launch reads as ONE experiment row instead of N sibling
 * rows, with the cells available behind an expand chevron.
 *
 * Everything here is pure over the run rows the list already fetched — the
 * pagination caveat applies: a cluster only sees the cells inside the current
 * fetch window. Authoritative cell counts come from the group API
 * (GET /v2/comparison-groups/:id); when that 404s (group deleted — the
 * test_run FK is ON DELETE SET NULL, so cells survive detached) callers fall
 * back to the in-window count flagged `approximate`.
 */

import type { TestRun } from '../../api/types';
import { runDisplayStatus } from '../../lib/runStatus';
import { stripCellNameSuffix } from './compare';

export type RunListRow<T extends TestRun = TestRun> =
  | { kind: 'run'; run: T; inGroup?: boolean }
  | { kind: 'group'; groupId: string; runs: T[] };

const ACTIVE_STATUSES: ReadonlySet<string> = new Set(['queued', 'provisioning', 'running']);

function isActive(run: Pick<TestRun, 'status'>): boolean {
  return ACTIVE_STATUSES.has(run.status);
}

/**
 * Cluster runs sharing a `comparison_group_id` into a single group row, kept
 * at the position of the group's first (newest) member so the list stays in
 * server order. Standalone runs pass through untouched.
 */
export function clusterComparisonGroups<T extends TestRun>(runs: T[]): RunListRow<T>[] {
  const rows: RunListRow<T>[] = [];
  const groupRows = new Map<string, { kind: 'group'; groupId: string; runs: T[] }>();
  for (const run of runs) {
    const groupId = run.comparison_group_id;
    if (!groupId) {
      rows.push({ kind: 'run', run });
      continue;
    }
    let group = groupRows.get(groupId);
    if (!group) {
      group = { kind: 'group', groupId, runs: [] };
      groupRows.set(groupId, group);
      rows.push(group);
    }
    group.runs.push(run);
  }
  return rows;
}

export interface GroupProgress {
  /** Cells that reached a terminal state (completed/failed/cancelled). */
  terminal: number;
  /** Cells whose verdict is failed (status failed, or completed all-failed). */
  failed: number;
  /** Cell count — authoritative when the group API supplied it. */
  total: number;
  /** True when `total` is only the in-window count (group API 404/pending). */
  approximate: boolean;
}

/**
 * `X/N · F failed` numbers for a group row. `definedCellCount` is
 * `group.cells.length` from the group API; pass null while it is loading or
 * after a 404 (deleted group) to fall back to the in-window count, flagged
 * approximate so the UI can render `X/N+`.
 */
export function groupProgress(
  runs: Array<Pick<TestRun, 'status' | 'success_count' | 'failure_count'>>,
  definedCellCount: number | null,
): GroupProgress {
  let terminal = 0;
  let failed = 0;
  for (const run of runs) {
    if (!isActive(run)) terminal += 1;
    if (runDisplayStatus(run) === 'failed') failed += 1;
  }
  // A group can hold more runs than defined cells (relaunches); never report
  // a total below what is visibly in the window.
  const total = definedCellCount !== null ? Math.max(definedCellCount, runs.length) : runs.length;
  return { terminal, failed, total, approximate: definedCellCount === null };
}

/**
 * One status chip for the whole group:
 * - any cell still active                  → running (pulses)
 * - all terminal, none failed              → completed (or cancelled when
 *                                            every cell was cancelled)
 * - all terminal, every cell failed        → failed
 * - all terminal, mixed verdicts           → partial
 */
export function groupAggregateStatus(
  runs: Array<Pick<TestRun, 'status' | 'success_count' | 'failure_count'>>,
): string {
  if (runs.length === 0) return 'queued';
  if (runs.some(isActive)) return 'running';
  let failed = 0;
  let cancelled = 0;
  for (const run of runs) {
    const verdict = runDisplayStatus(run);
    if (verdict === 'failed') failed += 1;
    else if (verdict === 'cancelled') cancelled += 1;
  }
  if (failed === runs.length) return 'failed';
  if (cancelled === runs.length) return 'cancelled';
  if (failed > 0) return 'partial';
  return 'completed';
}

/**
 * Fallback group name from the cells' shared config-name prefix (suffix-
 * stripped), for groups the API no longer knows (deleted) or has not loaded
 * yet. Returns null when the cells share no meaningful prefix — callers
 * degrade to `Group <shortid>`.
 */
export function groupFallbackName(
  runs: Array<Pick<TestRun, 'config_name'>>,
): string | null {
  const names = runs
    .map((r) => (r.config_name ? stripCellNameSuffix(r.config_name) : ''))
    .filter(Boolean);
  if (names.length === 0) return null;
  let prefix = names[0];
  for (const name of names.slice(1)) {
    let i = 0;
    while (i < prefix.length && i < name.length && prefix[i] === name[i]) i += 1;
    prefix = prefix.slice(0, i);
    if (!prefix) return null;
  }
  // Trim a dangling separator/word fragment ("azure/eastus linux · " → the
  // clean shared part, "go @ az" vs "go @ aws" → "go @").
  const trimmed = prefix.replace(/[\s·@/,-]+$/u, '').trim();
  return trimmed.length >= 3 ? trimmed : null;
}

/**
 * Stable cell ordering for prev/next navigation: by suffix-stripped config
 * name (the cell label — consistent with the compare page's label-sorted
 * sections), id as tiebreak so relaunched twins don't jitter.
 */
export function sortCellsByName<T extends TestRun>(runs: T[]): T[] {
  return [...runs].sort((a, b) => {
    const an = stripCellNameSuffix(a.config_name ?? '');
    const bn = stripCellNameSuffix(b.config_name ?? '');
    return an.localeCompare(bn) || a.id.localeCompare(b.id);
  });
}
