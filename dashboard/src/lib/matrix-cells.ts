import type { ComparisonCell } from '../api/types';
import type { TestbedState } from '../components/wizard/testbed-constants';
import { isH3Mode, stackHasH3 } from './http-stacks';
import {
  PROXY_LABELS,
  resolveVmSize,
  resolveTopology,
} from '../components/wizard/testbed-constants';

/**
 * Fan out each testbed across its selected proxies into comparison-group
 * cells. One testbed with [nginx, caddy] becomes 2 cells — and 2 VMs: the
 * orchestrator provisions ONE VM PER LAUNCHED RUN
 * (ProvisioningOrchestrator.KickOneAsync); there is no dedup by
 * (cloud_account_id, region, vm_size, os). An earlier comment here claimed
 * cells "share one deployment" — that code never existed, and the claim
 * leaked into the UI as a cost undercount (#793 P2-1).
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

/** One cell's HTTP/3 trim: the modes the server will NOT run on it. */
export interface CellModeDrop {
  /** Cell label as buildComparisonCells() names it. */
  label: string;
  stack: string;
  dropped: string[];
}

/**
 * Per-cell HTTP/3 drop preview — mirrors what the comparison-group launch does
 * server-side (ComparisonGroupsEndpoints.TrimH3ModesForCell): on a cell whose
 * proxy stack has no QUIC (shared/http-stacks.json h3=false), the h3 modes are
 * dropped for THAT cell only and the group still runs. A matrix over nginx +
 * apache runs http3 on the nginx cell and skips it on the apache cell.
 * Returns only cells that lose something; empty when nothing is dropped.
 */
export function h3DropsPerCell(testbeds: TestbedState[], modes: Iterable<string>): CellModeDrop[] {
  const selected = [...modes];
  const h3 = selected.filter(isH3Mode);
  if (h3.length === 0) return [];
  const drops: CellModeDrop[] = [];
  for (const tb of testbeds) {
    for (const proxy of tb.proxies) {
      if (stackHasH3(proxy) === false) {
        drops.push({
          label: `${tb.cloud}/${tb.region} ${tb.os} · ${PROXY_LABELS[proxy] ?? proxy}`,
          stack: proxy,
          dropped: h3,
        });
      }
    }
  }
  return drops;
}
