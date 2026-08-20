// URL Probe provider / capacity grouping. The page used to fold every run on
// a host into one row regardless of which runner probed it, so an Azure B1s
// and a GCP e2-standard-4 averaged into one "microsoft.com" verdict. These
// pin the bucket keys (host-only default unchanged; provider and capacity
// splits; the explicit unknown-runner bucket), the run → tester fallback
// chain, the filter predicate, and the capacity option ordering.

import { describe, expect, it } from 'vitest';
import {
  BUCKET_KEY_SEP,
  NO_RUNNER,
  UNKNOWN_RUNNER,
  capacityLabel,
  capacityOptions,
  matchesRunnerFilter,
  parseProbeGroupBy,
  probeBucketFor,
  providerOptions,
  runnerIdentityForRun,
  summarizeBucketRunners,
  type RunnerIdentity,
  type RunnerRunLike,
  type TesterLike,
} from './probe-grouping';

const azureB1s: RunnerIdentity = {
  cloud: 'azure', region: 'westeurope', vmSize: 'Standard_B1s', vcpus: 1, memoryGb: 1,
};
const gcpMedium: RunnerIdentity = {
  cloud: 'gcp', region: 'us-central1', vmSize: 'e2-medium', vcpus: 2, memoryGb: 4,
};
const gcpStd4: RunnerIdentity = {
  cloud: 'gcp', region: 'europe-west1', vmSize: 'e2-standard-4', vcpus: 4, memoryGb: 16,
};

function run(overrides: Partial<RunnerRunLike> = {}): RunnerRunLike {
  return {
    tester_id: null,
    runner_cloud: null,
    runner_region: null,
    runner_vm_size: null,
    runner_vcpus: null,
    runner_memory_gb: null,
    ...overrides,
  };
}

describe('parseProbeGroupBy', () => {
  it('defaults to host for anything but the two grouped modes', () => {
    expect(parseProbeGroupBy(null)).toBe('host');
    expect(parseProbeGroupBy(undefined)).toBe('host');
    expect(parseProbeGroupBy('')).toBe('host');
    expect(parseProbeGroupBy('host')).toBe('host');
    expect(parseProbeGroupBy('region')).toBe('host');
    expect(parseProbeGroupBy('provider')).toBe('provider');
    expect(parseProbeGroupBy('capacity')).toBe('capacity');
  });
});

describe('runnerIdentityForRun', () => {
  const testers = new Map<string, TesterLike>([
    ['t-1', { cloud: 'Azure', region: 'westeurope', vm_size: 'Standard_B1s' }],
  ]);

  it('prefers the denormalized runner_* fields on the run', () => {
    const id = runnerIdentityForRun(run({
      tester_id: 't-1',
      runner_cloud: 'gcp',
      runner_region: 'us-central1',
      runner_vm_size: 'e2-medium',
      runner_vcpus: 2,
      runner_memory_gb: 4,
    }), testers);
    expect(id).toEqual(gcpMedium);
  });

  it('falls back per field to the loaded testers map by tester_id', () => {
    // Older control plane / a list item without the fields: identity comes
    // from the tester row; specs stay null (the testers DTO has none).
    const id = runnerIdentityForRun(run({ tester_id: 't-1' }), testers);
    expect(id).toEqual({
      cloud: 'azure', region: 'westeurope', vmSize: 'Standard_B1s', vcpus: null, memoryGb: null,
    });
    // Partial: run carries the cloud only, tester fills the rest.
    const partial = runnerIdentityForRun(run({ tester_id: 't-1', runner_cloud: 'azure' }), testers);
    expect(partial.vmSize).toBe('Standard_B1s');
  });

  it('is NO_RUNNER when neither source knows the runner', () => {
    expect(runnerIdentityForRun(run(), testers)).toEqual(NO_RUNNER);
    expect(runnerIdentityForRun(run({ tester_id: 'deleted' }), testers)).toEqual(NO_RUNNER);
    // Blank strings are not an identity either.
    expect(runnerIdentityForRun(run({ runner_cloud: '  ', runner_vm_size: '' }), testers)).toEqual(NO_RUNNER);
  });
});

describe('probeBucketFor', () => {
  it('host grouping (default): one bucket per host regardless of runner', () => {
    const a = probeBucketFor('microsoft.com', 'host', azureB1s);
    const b = probeBucketFor('microsoft.com', 'host', gcpStd4);
    const c = probeBucketFor('microsoft.com', 'host', NO_RUNNER);
    expect(a.key).toBe('microsoft.com');
    expect(b.key).toBe(a.key);
    expect(c.key).toBe(a.key);
    expect(a.cloud).toBeNull();
    expect(a.vmSize).toBeNull();
    expect(c.unknownRunner).toBe(false);
  });

  it('provider grouping splits by cloud and ignores the size', () => {
    const azure = probeBucketFor('microsoft.com', 'provider', azureB1s);
    const gcpA = probeBucketFor('microsoft.com', 'provider', gcpMedium);
    const gcpB = probeBucketFor('microsoft.com', 'provider', gcpStd4);
    expect(azure.key).toBe(`microsoft.com${BUCKET_KEY_SEP}azure`);
    expect(gcpA.key).toBe(`microsoft.com${BUCKET_KEY_SEP}gcp`);
    expect(gcpB.key).toBe(gcpA.key);
    expect(azure.cloud).toBe('azure');
    expect(azure.vmSize).toBeNull();
    expect(azure.unknownRunner).toBe(false);
    // A different host never shares a bucket.
    expect(probeBucketFor('github.com', 'provider', azureB1s).key).not.toBe(azure.key);
  });

  it('capacity grouping splits by cloud AND size', () => {
    const gcpA = probeBucketFor('microsoft.com', 'capacity', gcpMedium);
    const gcpB = probeBucketFor('microsoft.com', 'capacity', gcpStd4);
    expect(gcpA.key).toBe(`microsoft.com${BUCKET_KEY_SEP}gcp${BUCKET_KEY_SEP}e2-medium`);
    expect(gcpB.key).toBe(`microsoft.com${BUCKET_KEY_SEP}gcp${BUCKET_KEY_SEP}e2-standard-4`);
    expect(gcpA.key).not.toBe(gcpB.key);
    expect(gcpA.cloud).toBe('gcp');
    expect(gcpA.vmSize).toBe('e2-medium');
  });

  it('an unresolved runner lands in its own explicit bucket, never a real one', () => {
    const unknownP = probeBucketFor('microsoft.com', 'provider', NO_RUNNER);
    expect(unknownP.key).toBe(`microsoft.com${BUCKET_KEY_SEP}${UNKNOWN_RUNNER}`);
    expect(unknownP.unknownRunner).toBe(true);
    expect(unknownP.cloud).toBeNull();
    expect(unknownP.key).not.toBe(probeBucketFor('microsoft.com', 'provider', azureB1s).key);

    const unknownC = probeBucketFor('microsoft.com', 'capacity', NO_RUNNER);
    expect(unknownC.key).toBe(`microsoft.com${BUCKET_KEY_SEP}${UNKNOWN_RUNNER}${BUCKET_KEY_SEP}${UNKNOWN_RUNNER}`);
    expect(unknownC.unknownRunner).toBe(true);

    // Cloud known but size not: a distinct "gcp / unknown size" row — not
    // merged into gcp/e2-medium, and not flagged as a fully unknown runner.
    const sizeless = probeBucketFor('microsoft.com', 'capacity', { ...gcpMedium, vmSize: null });
    expect(sizeless.key).toBe(`microsoft.com${BUCKET_KEY_SEP}gcp${BUCKET_KEY_SEP}${UNKNOWN_RUNNER}`);
    expect(sizeless.key).not.toBe(probeBucketFor('microsoft.com', 'capacity', gcpMedium).key);
    expect(sizeless.unknownRunner).toBe(false);
  });
});

describe('summarizeBucketRunners', () => {
  it('collects distinct regions and the first known specs', () => {
    const s = summarizeBucketRunners([
      { ...gcpMedium, vcpus: null, memoryGb: null },
      gcpMedium,
      { ...gcpMedium, region: 'europe-west1' },
      { ...gcpMedium, region: null },
    ]);
    expect(s.regions).toEqual(['europe-west1', 'us-central1']);
    expect(s.vcpus).toBe(2);
    expect(s.memoryGb).toBe(4);
    expect(summarizeBucketRunners([NO_RUNNER])).toEqual({ regions: [], vcpus: null, memoryGb: null });
  });
});

describe('matchesRunnerFilter', () => {
  it('filters by provider and size, with the unknown sentinel selecting unresolved runs', () => {
    expect(matchesRunnerFilter(gcpMedium, null, null)).toBe(true);
    expect(matchesRunnerFilter(gcpMedium, '', '')).toBe(true);
    expect(matchesRunnerFilter(gcpMedium, 'gcp', null)).toBe(true);
    expect(matchesRunnerFilter(gcpMedium, 'azure', null)).toBe(false);
    expect(matchesRunnerFilter(gcpMedium, 'gcp', 'e2-medium')).toBe(true);
    expect(matchesRunnerFilter(gcpMedium, 'gcp', 'e2-standard-4')).toBe(false);
    expect(matchesRunnerFilter(NO_RUNNER, UNKNOWN_RUNNER, null)).toBe(true);
    expect(matchesRunnerFilter(gcpMedium, UNKNOWN_RUNNER, null)).toBe(false);
    expect(matchesRunnerFilter(NO_RUNNER, null, UNKNOWN_RUNNER)).toBe(true);
    expect(matchesRunnerFilter(NO_RUNNER, 'gcp', null)).toBe(false);
  });
});

describe('providerOptions', () => {
  it('lists distinct clouds A→Z with counts, unknown last only when present', () => {
    expect(providerOptions([gcpMedium, azureB1s, gcpStd4])).toEqual([
      { value: 'azure', label: 'azure', count: 1 },
      { value: 'gcp', label: 'gcp', count: 2 },
    ]);
    expect(providerOptions([gcpMedium, NO_RUNNER])).toEqual([
      { value: 'gcp', label: 'gcp', count: 1 },
      { value: UNKNOWN_RUNNER, label: 'unknown runner', count: 1 },
    ]);
    expect(providerOptions([])).toEqual([]);
  });
});

describe('capacityOptions', () => {
  it('labels sizes with their catalog numbers and sorts by vCPU, memory, then name', () => {
    const t3small: RunnerIdentity = { cloud: 'aws', region: 'eu-west-1', vmSize: 't3.small', vcpus: 2, memoryGb: 2 };
    const uncatalogued: RunnerIdentity = { cloud: 'azure', region: 'westeurope', vmSize: 'Standard_Z99_v9', vcpus: null, memoryGb: null };
    const options = capacityOptions([
      gcpStd4, uncatalogued, gcpMedium, NO_RUNNER, azureB1s, t3small, gcpMedium,
    ]);
    expect(options.map(o => o.value)).toEqual([
      'Standard_B1s',      // 1 vCPU / 1 GB
      't3.small',          // 2 vCPU / 2 GB
      'e2-medium',         // 2 vCPU / 4 GB
      'e2-standard-4',     // 4 vCPU / 16 GB
      'Standard_Z99_v9',   // no catalog numbers → after every known size
      UNKNOWN_RUNNER,      // unresolved size → last
    ]);
    expect(options[2]).toEqual({ value: 'e2-medium', label: 'e2-medium · 2 vCPU / 4 GB', count: 2 });
    expect(options[4].label).toBe('Standard_Z99_v9');
    expect(options[5].label).toBe('unknown size');
  });

  it('breaks a vCPU tie by memory and a full tie by name', () => {
    const a: RunnerIdentity = { cloud: 'gcp', region: null, vmSize: 'n2-standard-2', vcpus: 2, memoryGb: 8 };
    const b: RunnerIdentity = { cloud: 'gcp', region: null, vmSize: 'e2-standard-2', vcpus: 2, memoryGb: 8 };
    const c: RunnerIdentity = { cloud: 'aws', region: null, vmSize: 't3.medium', vcpus: 2, memoryGb: 4 };
    expect(capacityOptions([a, b, c]).map(o => o.value)).toEqual(['t3.medium', 'e2-standard-2', 'n2-standard-2']);
  });

  it('capacityLabel falls back to the bare size without catalog numbers', () => {
    expect(capacityLabel('e2-medium', 2, 4)).toBe('e2-medium · 2 vCPU / 4 GB');
    expect(capacityLabel('custom-1', null, null)).toBe('custom-1');
    expect(capacityLabel('custom-2', 2, null)).toBe('custom-2');
  });
});
