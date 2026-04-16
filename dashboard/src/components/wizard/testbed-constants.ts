// ── Shared constants for testbed-based wizards ─────────────────────────

export const CLOUDS = ['Azure', 'AWS', 'GCP'] as const;

export const REGIONS: Record<string, string[]> = {
  Azure: ['eastus', 'eastus2', 'westus2', 'westus3', 'centralus', 'northeurope', 'westeurope', 'southeastasia', 'japaneast', 'australiaeast'],
  AWS: ['us-east-1', 'us-east-2', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1', 'ap-northeast-1', 'ap-southeast-2'],
  GCP: ['us-central1', 'us-east1', 'us-west1', 'europe-west1', 'europe-west4', 'asia-southeast1', 'asia-northeast1', 'australia-southeast1'],
};

export const TOPOLOGIES = ['Loopback', 'Same-region'] as const;
export const VM_SIZES = ['Small', 'Medium', 'Large'] as const;

export const LINUX_PROXIES = ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'] as const;
export const WINDOWS_PROXIES = ['iis', 'nginx', 'caddy', 'traefik', 'haproxy', 'apache'] as const;

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
  region: string;
  topology: string;
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
    region: REGIONS[c]?.[0] ?? '',
    topology: 'Loopback',
    vmSize: 'Medium',
    os: os ?? 'linux',
    proxies: proxies ?? [],
    testerOs: 'server',
    existingVm: false,
    existingVmId: '',
  };
}

export function updateTestbedState(testbeds: TestbedState[], key: number, patch: Partial<TestbedState>): TestbedState[] {
  return testbeds.map(c => {
    if (c.key !== key) return c;
    const updated = { ...c, ...patch };
    if (patch.cloud && patch.cloud !== c.cloud) {
      updated.region = REGIONS[patch.cloud]?.[0] ?? '';
    }
    if (patch.os && patch.os !== c.os) {
      const validProxies = patch.os === 'windows'
        ? WINDOWS_PROXIES as readonly string[]
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

export function requiresWindows(langs: Set<string>): boolean {
  return [...langs].some(id => WINDOWS_ONLY_LANGS.has(id));
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
    description: 'IIS + nginx proxies, .NET ecosystem.',
    defaultTestbedCount: 1,
    defaultOs: 'windows',
    defaultLanguages: ['nginx', 'csharp-net48', 'csharp-net8', 'csharp-net8-aot', 'csharp-net9', 'csharp-net9-aot'],
    defaultProxies: ['iis', 'nginx'],
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

// ── Deploy regions (for proxy target creation) ─────────────────────────

export const DEPLOY_REGIONS: Record<string, string[]> = {
  azure: ['eastus', 'westus2', 'westeurope', 'northeurope', 'southeastasia'],
  aws: ['us-east-1', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1'],
  gcp: ['us-central1', 'us-east1', 'europe-west1', 'europe-west4', 'asia-southeast1'],
};

export const HTTP_STACKS = ['nginx', 'iis'] as const;
