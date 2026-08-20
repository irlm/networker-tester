/**
 * Provider / capacity grouping for the URL Probe page.
 *
 * A watched host's history used to be ONE row no matter which runner probed
 * it — microsoft.com averaged an Azure B1s in westeurope against a GCP
 * e2-standard-4 in us-central1, and the spread read as a network-path
 * difference when it was a runner-infrastructure difference. These helpers
 * split a host's runs along two extra axes:
 *
 *   host      — the default; one row per host (unchanged behaviour)
 *   provider  — one row per host × runner cloud
 *   capacity  — one row per host × runner cloud × VM size
 *
 * The runner identity comes from the run-list's denormalized `runner_*`
 * fields first; when a field is null (older control plane, or a run whose
 * tester row was deleted) the already-loaded testers map fills it in by
 * `tester_id`. A run with neither source lands in an explicit "unknown
 * runner" bucket — it is never merged into a real provider/size row, because
 * a silently mis-attributed run is exactly the comparison error this exists
 * to remove.
 *
 * Pure functions only (no React) so the bucket keys and option ordering are
 * unit-tested without the page.
 */

import type { TestRun } from '../api/types';

export type ProbeGroupBy = 'host' | 'provider' | 'capacity';

export const PROBE_GROUP_BY_VALUES: readonly ProbeGroupBy[] = ['host', 'provider', 'capacity'];

/** `?group=` parser — anything but the two grouped modes is the host default. */
export function parseProbeGroupBy(value: string | null | undefined): ProbeGroupBy {
  return value === 'provider' || value === 'capacity' ? value : 'host';
}

/**
 * Sentinel for "the runner could not be resolved" in bucket keys, filter
 * option values and the `?provider=` / `?size=` query params. No cloud or VM
 * size is literally named this.
 */
export const UNKNOWN_RUNNER = 'unknown';

/** Separator inside a bucket key; neither hostnames nor VM sizes contain it. */
export const BUCKET_KEY_SEP = '::';

export interface RunnerIdentity {
  /** Lower-cased provider id (`azure` / `aws` / `gcp` / `docker`), null when unknown. */
  cloud: string | null;
  region: string | null;
  vmSize: string | null;
  /** Catalog specs (VmNetworkSpecs via the list route); null when unknown. */
  vcpus: number | null;
  memoryGb: number | null;
}

export const NO_RUNNER: RunnerIdentity = Object.freeze({
  cloud: null,
  region: null,
  vmSize: null,
  vcpus: null,
  memoryGb: null,
});

/** The list-item fields this module reads (a TestRun satisfies it). */
export type RunnerRunLike = Pick<
  TestRun,
  'tester_id' | 'runner_cloud' | 'runner_region' | 'runner_vm_size' | 'runner_vcpus' | 'runner_memory_gb'
>;

/** The tester-row fields used as the fallback source (a TesterRow satisfies it). */
export interface TesterLike {
  cloud: string;
  region: string;
  vm_size: string;
}

function nonEmpty(value: string | null | undefined): string | null {
  if (value == null) return null;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/**
 * Resolve a run's runner identity: the run's own denormalized fields win,
 * the testers map fills any null field by `tester_id`, and a run with
 * neither is {@link NO_RUNNER}. Specs come only from the run (the testers
 * list DTO carries no catalog numbers).
 */
export function runnerIdentityForRun(
  run: RunnerRunLike,
  testersById: ReadonlyMap<string, TesterLike>,
): RunnerIdentity {
  const tester = run.tester_id ? testersById.get(run.tester_id) : undefined;
  const cloud = nonEmpty(run.runner_cloud) ?? nonEmpty(tester?.cloud);
  return {
    cloud: cloud ? cloud.toLowerCase() : null,
    region: nonEmpty(run.runner_region) ?? nonEmpty(tester?.region),
    vmSize: nonEmpty(run.runner_vm_size) ?? nonEmpty(tester?.vm_size),
    vcpus: run.runner_vcpus ?? null,
    memoryGb: run.runner_memory_gb ?? null,
  };
}

export interface ProbeBucket {
  /**
   * Stable row key: the host alone for `host`; host + cloud for `provider`;
   * host + cloud + size for `capacity`. An unresolved dimension is the
   * {@link UNKNOWN_RUNNER} sentinel, so unknown runs form their own row.
   */
  key: string;
  host: string;
  groupBy: ProbeGroupBy;
  /** Null under `host` grouping (the axis is not in play), else the resolved cloud or null = unknown. */
  cloud: string | null;
  /** Null unless `capacity` grouping, else the resolved size or null = unknown. */
  vmSize: string | null;
  /** True when the grouping axis could not be resolved for this run. */
  unknownRunner: boolean;
}

/** The bucket a run on `host` belongs to under `groupBy`. */
export function probeBucketFor(host: string, groupBy: ProbeGroupBy, runner: RunnerIdentity): ProbeBucket {
  if (groupBy === 'host') {
    return { key: host, host, groupBy, cloud: null, vmSize: null, unknownRunner: false };
  }
  const cloudPart = runner.cloud ?? UNKNOWN_RUNNER;
  if (groupBy === 'provider') {
    return {
      key: [host, cloudPart].join(BUCKET_KEY_SEP),
      host,
      groupBy,
      cloud: runner.cloud,
      vmSize: null,
      unknownRunner: runner.cloud === null,
    };
  }
  const sizePart = runner.vmSize ?? UNKNOWN_RUNNER;
  return {
    key: [host, cloudPart, sizePart].join(BUCKET_KEY_SEP),
    host,
    groupBy,
    cloud: runner.cloud,
    vmSize: runner.vmSize,
    unknownRunner: runner.cloud === null && runner.vmSize === null,
  };
}

export interface BucketRunnerSummary {
  /** Distinct regions seen in the bucket (a capacity row can span regions — region is not part of its key). */
  regions: string[];
  vcpus: number | null;
  memoryGb: number | null;
}

/** Header facts for a bucket, aggregated over its runs' identities. */
export function summarizeBucketRunners(identities: readonly RunnerIdentity[]): BucketRunnerSummary {
  const regions = new Set<string>();
  let vcpus: number | null = null;
  let memoryGb: number | null = null;
  for (const id of identities) {
    if (id.region) regions.add(id.region);
    if (vcpus === null && id.vcpus !== null) vcpus = id.vcpus;
    if (memoryGb === null && id.memoryGb !== null) memoryGb = id.memoryGb;
  }
  return { regions: [...regions].sort(), vcpus, memoryGb };
}

/**
 * Provider / capacity filter predicate. A filter value of
 * {@link UNKNOWN_RUNNER} selects the runs whose dimension is unresolved; null
 * or '' means "all".
 */
export function matchesRunnerFilter(
  runner: RunnerIdentity,
  provider: string | null,
  size: string | null,
): boolean {
  if (provider) {
    if (provider === UNKNOWN_RUNNER ? runner.cloud !== null : runner.cloud !== provider) return false;
  }
  if (size) {
    if (size === UNKNOWN_RUNNER ? runner.vmSize !== null : runner.vmSize !== size) return false;
  }
  return true;
}

export interface RunnerFilterOption {
  value: string;
  label: string;
  /** Runs carrying this value among the identities the options were built from. */
  count: number;
}

/** Distinct providers present, A→Z, with the unknown bucket last when any run needs it. */
export function providerOptions(identities: readonly RunnerIdentity[]): RunnerFilterOption[] {
  const counts = new Map<string, number>();
  let unknown = 0;
  for (const id of identities) {
    if (id.cloud === null) unknown += 1;
    else counts.set(id.cloud, (counts.get(id.cloud) ?? 0) + 1);
  }
  const options = [...counts.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([value, count]) => ({ value, label: value, count }));
  if (unknown > 0) options.push({ value: UNKNOWN_RUNNER, label: 'unknown runner', count: unknown });
  return options;
}

/** `e2-medium · 2 vCPU / 4 GB`, or the bare size when the catalog has no entry. */
export function capacityLabel(vmSize: string, vcpus: number | null, memoryGb: number | null): string {
  return vcpus !== null && memoryGb !== null
    ? `${vmSize} · ${vcpus} vCPU / ${memoryGb} GB`
    : vmSize;
}

/**
 * Distinct VM sizes present, smallest first: by vCPU, then memory, then
 * name; sizes without catalog numbers sort after every known size (by
 * name); the unknown bucket is last when any run needs it. Pass the
 * identities already narrowed by the provider filter so the list only
 * offers sizes that exist under that provider.
 */
export function capacityOptions(identities: readonly RunnerIdentity[]): RunnerFilterOption[] {
  const bySize = new Map<string, { vcpus: number | null; memoryGb: number | null; count: number }>();
  let unknown = 0;
  for (const id of identities) {
    if (id.vmSize === null) {
      unknown += 1;
      continue;
    }
    const entry = bySize.get(id.vmSize);
    if (entry) {
      entry.count += 1;
      if (entry.vcpus === null) entry.vcpus = id.vcpus;
      if (entry.memoryGb === null) entry.memoryGb = id.memoryGb;
    } else {
      bySize.set(id.vmSize, { vcpus: id.vcpus, memoryGb: id.memoryGb, count: 1 });
    }
  }
  const options = [...bySize.entries()]
    .sort(([aName, a], [bName, b]) => {
      const aKnown = a.vcpus !== null;
      const bKnown = b.vcpus !== null;
      if (aKnown !== bKnown) return aKnown ? -1 : 1;
      if (aKnown && bKnown && a.vcpus !== b.vcpus) return (a.vcpus ?? 0) - (b.vcpus ?? 0);
      if ((a.memoryGb ?? -1) !== (b.memoryGb ?? -1)) return (a.memoryGb ?? -1) - (b.memoryGb ?? -1);
      return aName.localeCompare(bName);
    })
    .map(([value, { vcpus, memoryGb, count }]) => ({
      value,
      label: capacityLabel(value, vcpus, memoryGb),
      count,
    }));
  if (unknown > 0) options.push({ value: UNKNOWN_RUNNER, label: 'unknown size', count: unknown });
  return options;
}
