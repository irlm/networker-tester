// ── Shared constants for testbed-based wizards ─────────────────────────

import type { CloudAccountSummary } from '../../api/types';
import { unsupportedReason } from '../../lib/mode-capabilities';

export const REGIONS: Record<string, string[]> = {
  Azure: ['eastus', 'eastus2', 'westus2', 'westus3', 'centralus', 'northeurope', 'westeurope', 'southeastasia', 'japaneast', 'australiaeast'],
  AWS: ['us-east-1', 'us-east-2', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1', 'ap-northeast-1', 'ap-southeast-2'],
  GCP: ['us-central1', 'us-east1', 'us-west1', 'europe-west1', 'europe-west4', 'asia-southeast1', 'asia-northeast1', 'australia-southeast1'],
  // Docker (local): one host, one "region". Offered only when the control
  // plane reports docker_provider (see hooks/useDockerProvider).
  Docker: ['local'],
};

/** Wizard cloud label for the docker provider (wire value: `docker`). */
export const DOCKER_CLOUD_LABEL = 'Docker';

/**
 * Wire provider (`azure`/`aws`/`gcp`/`docker`, lowercase) → wizard cloud label
 * (the key into REGIONS / INSTANCE_TYPES). Feeding the raw wire value into
 * those tables silently misses every lookup — the autoprovision path did
 * exactly that and produced Standard_B2s-on-AWS with an empty region (#793
 * P1-2), so any provider-to-table hop MUST go through here.
 */
export function providerToCloud(provider: string): string {
  const p = provider.toLowerCase();
  if (p === 'azure') return 'Azure';
  if (p === 'aws') return 'AWS';
  if (p === 'gcp') return 'GCP';
  if (p === 'docker') return DOCKER_CLOUD_LABEL;
  return 'Azure';
}

/**
 * Default cloud account for prefilled/autoprovision flows: the first ACTIVE
 * account after an active-first sort (then provider, then name — the same
 * order InfraDeployWizard presents), or null when no healthy account exists.
 * Never returns an unhealthy account: silently defaulting to an
 * alphabetically-first broken account is what launched #791's failed matrix.
 */
export function pickDefaultAccount(accounts: CloudAccountSummary[]): CloudAccountSummary | null {
  const sorted = [...accounts].sort((a, b) => {
    if (a.status === 'active' && b.status !== 'active') return -1;
    if (a.status !== 'active' && b.status === 'active') return 1;
    return a.provider.localeCompare(b.provider) || a.name.localeCompare(b.name);
  });
  const first = sorted[0];
  return first && first.status === 'active' ? first : null;
}

export const TOPOLOGIES = ['Loopback', 'Same-region'] as const;

// ── Cloud-native instance types (replaces abstract Small/Medium/Large) ──
//
// Each entry shows the SKU + a short hint so users don't need to know
// every cloud's size matrix by heart.

export interface InstanceType {
  id: string;
  hint: string;
}

export const INSTANCE_TYPES: Record<string, InstanceType[]> = {
  Azure: [
    { id: 'Standard_B1s',    hint: '1 vCPU · 1 GiB · burstable' },
    { id: 'Standard_B2s',    hint: '2 vCPU · 4 GiB · burstable' },
    { id: 'Standard_B2ms',   hint: '2 vCPU · 8 GiB · burstable' },
    { id: 'Standard_D2s_v3', hint: '2 vCPU · 8 GiB' },
    { id: 'Standard_D4s_v5', hint: '4 vCPU · 16 GiB' },
    { id: 'Standard_D8s_v5', hint: '8 vCPU · 32 GiB' },
  ],
  AWS: [
    { id: 't3.micro',  hint: '2 vCPU · 1 GiB · burstable' },
    { id: 't3.small',  hint: '2 vCPU · 2 GiB · burstable' },
    { id: 't3.medium', hint: '2 vCPU · 4 GiB · burstable' },
    { id: 't3.large',  hint: '2 vCPU · 8 GiB · burstable' },
    { id: 'm5.large',  hint: '2 vCPU · 8 GiB' },
    { id: 'm5.xlarge', hint: '4 vCPU · 16 GiB' },
  ],
  GCP: [
    { id: 'e2-micro',     hint: '2 vCPU · 1 GiB · shared' },
    { id: 'e2-small',     hint: '2 vCPU · 2 GiB · shared' },
    { id: 'e2-medium',    hint: '2 vCPU · 4 GiB · shared' },
    { id: 'e2-standard-2', hint: '2 vCPU · 8 GiB' },
    { id: 'e2-standard-4', hint: '4 vCPU · 16 GiB' },
    { id: 'e2-standard-8', hint: '8 vCPU · 32 GiB' },
  ],
  Docker: [
    { id: 'container', hint: 'shares the control-plane host' },
  ],
};

/** First instance type for a cloud — used as the default when an account is picked. */
export function defaultInstanceType(cloud: string): string {
  return INSTANCE_TYPES[cloud]?.[1]?.id ?? INSTANCE_TYPES[cloud]?.[0]?.id ?? 'Standard_B2s';
}

export const LINUX_PROXIES = ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'] as const;
// Windows proxy support by cloud:
//   - Azure: iis, caddy, traefik — install.sh invokes install.ps1 -Setup
//     <proxy> over `az vm run-command` (_azure_win_setup_proxy).
//     haproxy/apache are NOT offered: HAProxy ships no native Windows build
//     and Apache has no scriptable Windows binary source (Apache Lounge
//     serves an HTML decoy to all automated downloads — verified 2026-08-04).
//     The server launch gate (ComparisonGroupsEndpoints.UnsupportedComboReason)
//     rejects them with the same reasons; keep BOTH lists in sync — each side
//     has a pin test naming the other.
//   - AWS:   iis only — install.sh 0.27.27+ bootstraps IIS via UserData
//     PowerShell on Windows Server 2022 (_aws_win_endpoint_full_userdata).
//     Non-IIS proxies would need AWS SSM RunPowerShellScript equivalent.
//   - GCP:   Windows endpoint deploys via startup-script, but install.sh
//     does not yet run install.ps1 remotely on GCE Windows (see install.sh
//     "IIS setup for GCP Windows would need gcloud SSH — deferred for now").
//     Restricted to [] here pending parity.
// nginx is rejected by install.sh validation on any Windows endpoint.
export const WINDOWS_PROXIES_AZURE = ['iis', 'caddy', 'traefik'] as const;
export const WINDOWS_PROXIES_AWS = ['iis'] as const;
export const WINDOWS_PROXIES_GCP: readonly string[] = [];

/** @deprecated use windowsProxiesFor(cloud). Kept for call-site compatibility. */
export const WINDOWS_PROXIES = WINDOWS_PROXIES_AZURE;

export function windowsProxiesFor(cloud: string): readonly string[] {
  if (cloud === 'Azure') return WINDOWS_PROXIES_AZURE;
  if (cloud === 'AWS') return WINDOWS_PROXIES_AWS;
  return WINDOWS_PROXIES_GCP;
}

export const PROXY_LABELS: Record<string, string> = {
  nginx: 'nginx', iis: 'IIS', caddy: 'Caddy', traefik: 'Traefik', haproxy: 'HAProxy', apache: 'Apache',
};

export const TESTER_OS_OPTIONS = [
  { id: 'server', label: 'Server (headless)' },
  { id: 'desktop-linux', label: 'Desktop Linux' },
  { id: 'desktop-windows', label: 'Desktop Windows' },
] as const;

// ── Testbed state ───────────────────────────────────────────────────────

export interface TestbedState {
  key: number;
  cloud: string;
  /** Cloud account ID the testbed will be provisioned against. Empty = not picked yet. */
  cloudAccountId: string;
  region: string;
  topology: string;
  /** Abstract size ('Small'/'Medium'/'Large'); translated to cloud-native at submit. */
  vmSize: string;
  os: 'linux' | 'windows';
  proxies: string[];
  testerOs: string;
  existingVm: boolean;
  existingVmId: string;
}

export function makeTestbed(key: number, cloud?: string, os?: 'linux' | 'windows', proxies?: string[]): TestbedState {
  const c = cloud ?? 'Azure';
  return {
    key,
    cloud: c,
    cloudAccountId: '',
    region: REGIONS[c]?.[0] ?? '',
    topology: 'Loopback',
    vmSize: defaultInstanceType(c),
    os: os ?? 'linux',
    proxies: proxies ?? [],
    testerOs: 'server',
    existingVm: false,
    existingVmId: '',
  };
}

/**
 * Launch gate for the Review step: when any testbed's selected cloud account
 * is not active, return the reason Launch must stay disabled (mirrors the
 * server-side gates in ComparisonGroupsEndpoints / ProvisioningOrchestrator);
 * null when every selected account is healthy. Docker/unselected testbeds
 * (empty cloudAccountId) don't gate here — other validation owns those.
 */
export function unhealthyAccountLaunchBlock(
  testbeds: TestbedState[],
  accounts: CloudAccountSummary[],
): string | null {
  const byId = new Map(accounts.map(a => [a.account_id, a]));
  for (const tb of testbeds) {
    if (!tb.cloudAccountId) continue;
    const acct = byId.get(tb.cloudAccountId);
    if (acct && acct.status !== 'active') {
      const detail = acct.validation_error ? `: ${acct.validation_error}` : '';
      return `cloud account '${acct.name}' is in ${acct.status} state${detail} — fix credentials in Settings → Cloud accounts or pick a healthy account`;
    }
  }
  return null;
}

/**
 * Testbed prefilled from a cloud account (the autoprovision path). Routes the
 * wire provider through providerToCloud so the REGIONS / INSTANCE_TYPES
 * lookups resolve (#793 P1-2), and honors the account's default region only
 * when it is a valid region for that cloud.
 */
export function makeTestbedForAccount(
  key: number,
  acct: CloudAccountSummary,
  os?: 'linux' | 'windows',
  proxies?: string[],
): TestbedState {
  const cloud = providerToCloud(acct.provider);
  const tb = makeTestbed(key, cloud, os, proxies);
  tb.cloudAccountId = acct.account_id;
  if (acct.region_default && (REGIONS[cloud] ?? []).includes(acct.region_default)) {
    tb.region = acct.region_default;
  }
  return tb;
}

/** Review step index in the Full Stack wizard (Testbeds/Workload/Methodology/Review). */
export const FULL_STACK_REVIEW_STEP = 3;

/**
 * Where an ?autoprovision deep link may land after prefilling (#793 slice b):
 * each prior step's canNext predicate is evaluated with the PREFILLED values
 * and the jump stops on the first failing step — Review is reachable only
 * when every prior step passes (the wizard's nextHint machinery then explains
 * the stop). This also closes the ?modes=<all-unsupported> hole: when zero
 * modes survive the proxy-stack gate for the prefilled testbed, the jump
 * stops on Workload instead of reaching Review with nothing to launch (which
 * used to 422 raw on Launch).
 */
export function autoprovisionJumpStep(testbed: TestbedState, rawModes: Iterable<string>): number {
  // Step 0 (Testbeds) — same predicate as the wizard's canNext.
  if (testbed.cloudAccountId === '' || testbed.proxies.length === 0) return 0;
  // Step 1 (Workload) — at least one selected mode must be EFFECTIVE against
  // the prefilled proxy stacks (mirrors the page's derived selectedModes).
  const effective = [...rawModes].filter(
    m => unsupportedReason(m, { kind: 'endpoint', stack: testbed.proxies }) === null,
  );
  if (effective.length === 0) return 1;
  // Step 2 (Methodology) always passes — it has no canNext condition.
  return FULL_STACK_REVIEW_STEP;
}

/**
 * Pre-v0.28 the wizard kept an abstract size (Small/Medium/Large) and resolved
 * to a native SKU at submit. We now store the native SKU directly, so this is
 * a passthrough — kept for call-site stability.
 */
export function resolveVmSize(_cloud: string, sku: string): string {
  return sku;
}

// Topology label ('Loopback'/'Same-region') → canonical wire value.
export function resolveTopology(label: string): string {
  return label.toLowerCase().replace(/\s+/g, '-');
}

/**
 * Next unique testbed key: max existing key + 1. Testbed keys MUST be unique —
 * updateTestbedState patches by key, so two rows sharing one key mutate
 * together (user-caught: template-seeded row + "+ add testbed" row changed OS
 * in lockstep). The old scheme used two uncoordinated counters (the page's
 * template counter and a matrix-local counter re-initialized to
 * testbeds.length on every remount), which desynced after a template re-apply
 * or a remove + step navigation.
 */
export function nextTestbedKey(testbeds: TestbedState[]): number {
  return testbeds.reduce((max, t) => Math.max(max, t.key), -1) + 1;
}

export function updateTestbedState(testbeds: TestbedState[], key: number, patch: Partial<TestbedState>): TestbedState[] {
  return testbeds.map(c => {
    if (c.key !== key) return c;
    const updated = { ...c, ...patch };
    if (patch.cloud && patch.cloud !== c.cloud) {
      updated.region = REGIONS[patch.cloud]?.[0] ?? '';
      // Reset vmSize to a valid native SKU for the new provider; keeping the
      // previous (e.g. Standard_B2s) when switching to AWS would be invalid.
      updated.vmSize = defaultInstanceType(patch.cloud);
    }
    // If cloud OR os changed, prune proxies that aren't valid for the new combo.
    // Windows proxy support differs per cloud (Azure has all 5, AWS/GCP have 0).
    const cloudChanged = patch.cloud && patch.cloud !== c.cloud;
    const osChanged = patch.os && patch.os !== c.os;
    if (cloudChanged || osChanged) {
      const effectiveCloud = updated.cloud;
      const effectiveOs = updated.os;
      const validProxies = effectiveOs === 'windows'
        ? windowsProxiesFor(effectiveCloud)
        : LINUX_PROXIES as readonly string[];
      updated.proxies = updated.proxies.filter(p => validProxies.includes(p));
    }
    return updated;
  });
}

// ── Language catalog ────────────────────────────────────────────────────

export interface LanguageEntry { id: string; label: string; group: string }

export const LANGUAGE_GROUPS: { label: string; entries: LanguageEntry[] }[] = [
  {
    label: 'Systems',
    entries: [
      { id: 'rust', label: 'Rust', group: 'Systems' },
      { id: 'go', label: 'Go', group: 'Systems' },
      { id: 'cpp', label: 'C++', group: 'Systems' },
    ],
  },
  {
    label: 'Managed',
    entries: [
      { id: 'csharp-net48', label: 'C# .NET 4.8', group: 'Managed' },
      { id: 'csharp-net8', label: 'C# .NET 8', group: 'Managed' },
      { id: 'csharp-net8-aot', label: 'C# .NET 8 AOT', group: 'Managed' },
      { id: 'csharp-net9', label: 'C# .NET 9', group: 'Managed' },
      { id: 'csharp-net9-aot', label: 'C# .NET 9 AOT', group: 'Managed' },
      { id: 'csharp-net10', label: 'C# .NET 10', group: 'Managed' },
      { id: 'csharp-net10-aot', label: 'C# .NET 10 AOT', group: 'Managed' },
      { id: 'java', label: 'Java', group: 'Managed' },
    ],
  },
  {
    label: 'Scripting',
    entries: [
      { id: 'nodejs', label: 'Node.js', group: 'Scripting' },
      { id: 'python', label: 'Python', group: 'Scripting' },
      { id: 'ruby', label: 'Ruby', group: 'Scripting' },
      { id: 'php', label: 'PHP', group: 'Scripting' },
    ],
  },
  {
    label: 'Static',
    entries: [
      { id: 'nginx', label: 'nginx', group: 'Static' },
    ],
  },
];

export const ALL_LANGUAGE_IDS = LANGUAGE_GROUPS.flatMap(g => g.entries.map(e => e.id));
export const TOP_5_IDS = ['nginx', 'rust', 'go', 'csharp-net8', 'java'];
export const SYSTEMS_IDS = ['rust', 'go', 'cpp'];

export const WINDOWS_ONLY_LANGS = new Set(['csharp-net48']);

/** Languages install.ps1 -BenchmarkServer cannot deploy — Linux-only.
 *  Mirror of install.sh's per-OS validator sets and
 *  DeployConfigPreflight.cs (C#); keep all three in lockstep.
 *  - *-aot: Native AOT publish needs the VS C++ toolchain on Windows —
 *    multi-GB, far outside the provisioning budget (issue #801 pattern A:
 *    every net8-aot/net9-aot @ windows cell died "install.sh exited with
 *    code 1").
 *  - cpp (MSVC+boost build), ruby (devkit gem builds), php (swoole is
 *    Linux-only), rust/nginx (Linux install paths only). */
export const LINUX_ONLY_LANGS = new Set([
  'csharp-net8-aot', 'csharp-net9-aot', 'csharp-net10-aot',
  'cpp', 'ruby', 'php', 'rust', 'nginx',
]);

export function requiresWindows(langs: Set<string>): boolean {
  return [...langs].some(id => WINDOWS_ONLY_LANGS.has(id));
}

/** Whether a language may run on a testbed OS. Windows-only runtimes
 *  (.NET Framework 4.8) must never produce Linux cells, and Linux-only
 *  runtimes (.NET AOT variants, cpp/ruby/php/rust/nginx) must never produce
 *  Windows cells: such a cell always fails provisioning, burning a VM and a
 *  doomed run (net48 @ linux user-caught 2026-08-19 #800; AOT @ windows and
 *  net9-aot @ linux field-confirmed 2026-08-19 #801).
 *  Empty/absent language is always allowed. */
export function languageAllowedOnOs(lang: string, os: 'linux' | 'windows'): boolean {
  if (!lang) return true;
  if (WINDOWS_ONLY_LANGS.has(lang)) return os === 'windows';
  if (LINUX_ONLY_LANGS.has(lang)) return os === 'linux';
  return true;
}

// ── Methodology ─────────────────────────────────────────────────────────

export interface MethodologyPreset {
  id: string;
  label: string;
  warmup: number;
  measured: number;
  targetError: number | null;
  description: string;
}

export const METHODOLOGY_PRESETS: MethodologyPreset[] = [
  { id: 'quick', label: 'Quick', warmup: 5, measured: 10, targetError: null, description: 'Fast exploratory runs' },
  { id: 'standard', label: 'Standard', warmup: 10, measured: 50, targetError: 5, description: 'Balanced accuracy and speed' },
  { id: 'rigorous', label: 'Rigorous', warmup: 10, measured: 200, targetError: 2, description: 'Maximum statistical confidence' },
];

export const DEFAULT_METHODOLOGY = {
  warmup_runs: 5,
  measured_runs: 30,
  cooldown_ms: 500,
  target_error_pct: 2.0,
  outlier_policy: { policy: 'iqr' as const, k: 1.5 },
  quality_gates: { max_cv_pct: 5.0, min_samples: 10, max_noise_level: 0.1 },
  publication_gates: { max_failure_pct: 5.0, require_all_phases: true },
};

/**
 * Methodology state seeded from a preset. Wizards that default the preset
 * selector to e.g. 'standard' MUST seed their methodology state from this —
 * seeding from DEFAULT_METHODOLOGY (5/30/2%) while highlighting Standard
 * (10/50/5%) made the Review step contradict the Methodology step (audit F14).
 */
export function methodologyForPreset(presetId: string): typeof DEFAULT_METHODOLOGY {
  const p = METHODOLOGY_PRESETS.find(m => m.id === presetId);
  if (!p) return { ...DEFAULT_METHODOLOGY };
  return {
    ...DEFAULT_METHODOLOGY,
    warmup_runs: p.warmup,
    measured_runs: p.measured,
    target_error_pct: p.targetError ?? 0,
  };
}

// ── Runtime templates ──────────────────────────────────────────────────

export interface RuntimeTemplate {
  id: string;
  name: string;
  description: string;
  defaultTestbedCount: number;
  defaultOs: 'linux' | 'windows' | null;
  defaultLanguages: string[];
  defaultProxies: string[];
  defaultModes: string[];
  methodology: 'quick' | 'standard' | 'rigorous';
}

export const RUNTIME_TEMPLATES: RuntimeTemplate[] = [
  {
    id: 'linux-api-stack',
    name: 'Linux API Stack',
    description: 'nginx + Caddy proxies, top 6 languages.',
    defaultTestbedCount: 1,
    defaultOs: 'linux',
    defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'java', 'nodejs'],
    defaultProxies: ['nginx', 'caddy'],
    defaultModes: ['http1', 'http2', 'http3', 'download', 'upload'],
    methodology: 'standard',
  },
  {
    id: 'windows-api-stack',
    name: 'Windows API Stack',
    description: 'IIS + Caddy proxies, .NET ecosystem.',
    defaultTestbedCount: 1,
    defaultOs: 'windows',
    // Windows-deployable set only (install.ps1 -BenchmarkServer): the old
    // defaults seeded nginx + AOT variants, all Linux-only — every such cell
    // either got silently dropped or failed provisioning (#801 pattern A).
    defaultLanguages: ['csharp-net48', 'csharp-net8', 'csharp-net9', 'csharp-net10', 'java'],
    defaultProxies: ['iis', 'caddy'],
    defaultModes: ['http1', 'http2', 'http3', 'download', 'upload'],
    methodology: 'standard',
  },
  {
    id: 'proxy-comparison',
    name: 'Proxy Comparison',
    description: 'All OS-compatible proxies, 3 languages.',
    defaultTestbedCount: 1,
    defaultOs: 'linux',
    defaultLanguages: ['nginx', 'rust', 'python'],
    defaultProxies: ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'],
    defaultModes: ['http1', 'http2', 'http3', 'download', 'upload'],
    methodology: 'standard',
  },
  {
    id: 'api-compute',
    name: 'API Compute (apibench)',
    description: 'Measured /api/* JSON workloads: users, transform, aggregate, search, compress. nginx excluded (no /api/* suite).',
    defaultTestbedCount: 1,
    defaultOs: 'linux',
    defaultLanguages: ['rust', 'go', 'csharp-net8', 'java', 'nodejs', 'python'],
    defaultProxies: ['nginx'],
    defaultModes: ['http1', 'apibench'],
    methodology: 'standard',
  },
  {
    id: 'validation-run',
    name: 'Validation Run',
    description: 'Golden run: Rust + Python, h2 + h3. Validates measurement correctness.',
    defaultTestbedCount: 1,
    defaultOs: 'linux',
    defaultLanguages: ['rust', 'python'],
    defaultProxies: ['nginx'],
    defaultModes: ['http2', 'http3'],
    methodology: 'standard',
  },
  {
    id: 'low-noise',
    name: 'Low Noise',
    description: 'Single language, extended warmup. For regression tracking.',
    defaultTestbedCount: 1,
    defaultOs: 'linux',
    defaultLanguages: ['rust'],
    defaultProxies: ['nginx'],
    defaultModes: ['http2'],
    methodology: 'rigorous',
  },
  {
    id: 'custom',
    name: 'Custom',
    description: 'Start from scratch.',
    defaultTestbedCount: 0,
    defaultOs: null,
    defaultLanguages: [],
    defaultProxies: [],
    defaultModes: ['http2'],
    methodology: 'standard',
  },
];
