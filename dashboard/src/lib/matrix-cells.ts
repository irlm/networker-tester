import type { ComparisonCell } from '../api/types';
import type { TestbedState } from '../components/wizard/testbed-constants';
import {
  PROXY_LABELS,
  resolveVmSize,
  resolveTopology,
} from '../components/wizard/testbed-constants';

/**
 * Fan out each testbed across its selected proxies into comparison-group
 * cells. One testbed with [nginx, caddy] becomes 2 cells — the orchestrator
 * deduplicates by (cloud_account_id, region, vm_size, os) so they share one
 * deployment that installs both stacks side by side.
 *
 * Extracted from FullStackPage (audit P1-12) so it can be tested: the matrix
 * wizard — the feature whose end-to-end path was broken for the whole
 * v0.28.129-147 campaign — had NO test file at all, and this function is the
 * thing that decides how many cells exist, what each is labelled, and which
 * runner they pin to.
 */
export function buildComparisonCells(
  testbeds: TestbedState[],
  selectedTesterId: string | null,
): ComparisonCell[] {
  const cells: ComparisonCell[] = [];
  for (const tb of testbeds) {
    const vmSize = resolveVmSize(tb.cloud, tb.vmSize);
    const topology = resolveTopology(tb.topology);
    for (const proxy of tb.proxies) {
      cells.push({
        label: `${tb.cloud}/${tb.region} ${tb.os} · ${PROXY_LABELS[proxy] ?? proxy}`,
        endpoint: {
          kind: 'pending',
          cloud_account_id: tb.cloudAccountId,
          region: tb.region,
          vm_size: vmSize,
          os: tb.os,
          proxy_stack: proxy,
          topology,
        },
        ...(selectedTesterId ? { runner_id: selectedTesterId } : {}),
      });
    }
  }
  return cells;
}

/**
 * Total cells a testbed set produces. Any combination yielding more than one
 * cell is a matrix run (multiple testbeds, OR one testbed with several
 * proxies).
 */
export function countCells(testbeds: TestbedState[]): number {
  return testbeds.reduce((n, tb) => n + tb.proxies.length, 0);
}

export function isMatrixRun(testbeds: TestbedState[]): boolean {
  return countCells(testbeds) > 1;
}
