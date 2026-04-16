import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { testersApi, type TesterRow } from '../api/testers';
import type { EndpointRef, EndpointKind, Workload, Methodology, TestConfigCreate, ModeGroup, Deployment, ComparisonCell, ComparisonGroupCreate, CloudAccountSummary } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ModeSelector } from '../components/common/ModeSelector';
import { PayloadSelector } from '../components/common/PayloadSelector';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { THROUGHPUT_IDS } from '../lib/chart';

// ── Constants ────────────────────────────────────────────────────────────

type Step = 0 | 1 | 2 | 3 | 4 | 5;

const CLOUDS = ['Azure', 'AWS', 'GCP'] as const;

const REGIONS: Record<string, string[]> = {
  Azure: ['eastus', 'eastus2', 'westus2', 'westus3', 'centralus', 'northeurope', 'westeurope', 'southeastasia', 'japaneast', 'australiaeast'],
  AWS: ['us-east-1', 'us-east-2', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1', 'ap-northeast-1', 'ap-southeast-2'],
  GCP: ['us-central1', 'us-east1', 'us-west1', 'europe-west1', 'europe-west4', 'asia-southeast1', 'asia-northeast1', 'australia-southeast1'],
};

const TOPOLOGIES = ['Loopback', 'Same-region'] as const;
const VM_SIZES = ['Small', 'Medium', 'Large'] as const;

const LINUX_PROXIES = ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'] as const;
const WINDOWS_PROXIES = ['iis', 'nginx', 'caddy', 'traefik', 'haproxy', 'apache'] as const;

const PROXY_LABELS: Record<string, string> = {
  nginx: 'nginx', iis: 'IIS', caddy: 'Caddy', traefik: 'Traefik', haproxy: 'HAProxy', apache: 'Apache',
};

const TESTER_OS_OPTIONS = [
  { id: 'server', label: 'Server (headless)' },
  { id: 'desktop-linux', label: 'Desktop Linux' },
  { id: 'desktop-windows', label: 'Desktop Windows' },
] as const;

// ── Runtime template definitions ────────────────────────────────────────

interface RuntimeTemplate {
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

const RUNTIME_TEMPLATES: RuntimeTemplate[] = [
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

// ── Language catalog ────────────────────────────────────────────────────

interface LanguageEntry { id: string; label: string; group: string }

const LANGUAGE_GROUPS: { label: string; entries: LanguageEntry[] }[] = [
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

const ALL_LANGUAGE_IDS = LANGUAGE_GROUPS.flatMap(g => g.entries.map(e => e.id));
const TOP_5_IDS = ['nginx', 'rust', 'go', 'csharp-net8', 'java'];
const SYSTEMS_IDS = ['rust', 'go', 'cpp'];

const WINDOWS_ONLY_LANGS = new Set(['csharp-net48']);

function requiresWindows(langs: Set<string>): boolean {
  return [...langs].some(id => WINDOWS_ONLY_LANGS.has(id));
}

// ── Methodology presets ─────────────────────────────────────────────────

const DEFAULT_METHODOLOGY: Methodology = {
  warmup_runs: 5,
  measured_runs: 30,
  cooldown_ms: 500,
  target_error_pct: 2.0,
  outlier_policy: { policy: 'iqr', k: 1.5 },
  quality_gates: { max_cv_pct: 5.0, min_samples: 10, max_noise_level: 0.1 },
  publication_gates: { max_failure_pct: 5.0, require_all_phases: true },
};

interface MethodologyPreset {
  id: string;
  label: string;
  warmup: number;
  measured: number;
  targetError: number | null;
  description: string;
}

const METHODOLOGY_PRESETS: MethodologyPreset[] = [
  { id: 'quick', label: 'Quick', warmup: 5, measured: 10, targetError: null, description: 'Fast exploratory runs' },
  { id: 'standard', label: 'Standard', warmup: 10, measured: 50, targetError: 5, description: 'Balanced accuracy and speed' },
  { id: 'rigorous', label: 'Rigorous', warmup: 10, measured: 200, targetError: 2, description: 'Maximum statistical confidence' },
];

// ── Proxy deployment helpers ────────────────────────────────────────────

const DEPLOY_REGIONS: Record<string, string[]> = {
  azure: ['eastus', 'westus2', 'westeurope', 'northeurope', 'southeastasia'],
  aws: ['us-east-1', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1'],
  gcp: ['us-central1', 'us-east1', 'europe-west1', 'europe-west4', 'asia-southeast1'],
};

const HTTP_STACKS = ['nginx', 'iis'] as const;

function deploymentStatusClass(status: string): string {
  switch (status) {
    case 'running': return 'text-green-400 border-green-500/40 bg-green-500/5';
    case 'stopped': case 'stopping': return 'text-gray-400 border-gray-700 bg-gray-800/40';
    case 'error': case 'failed': return 'text-red-400 border-red-500/40 bg-red-500/5';
    case 'creating': case 'starting': return 'text-yellow-300 border-yellow-500/40 bg-yellow-500/5';
    default: return 'text-gray-400 border-gray-700';
  }
}

// ── Testbed state ───────────────────────────────────────────────────────

interface TestbedState {
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

function makeTestbed(key: number, cloud?: string, os?: 'linux' | 'windows', proxies?: string[]): TestbedState {
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

// ── Runner helpers ──────────────────────────────────────────────────────

function testerStatusClass(row: TesterRow): string {
  if (row.power_state === 'error') return 'text-red-400 border-red-500/40 bg-red-500/5';
  if (row.power_state === 'stopped' || row.power_state === 'stopping') return 'text-gray-400 border-gray-700 bg-gray-800/40';
  if (row.allocation === 'locked' || row.allocation === 'upgrading') return 'text-yellow-300 border-yellow-500/40 bg-yellow-500/5';
  if (row.power_state === 'running' && row.allocation === 'idle') return 'text-green-400 border-green-500/40 bg-green-500/5';
  return 'text-gray-400 border-gray-700';
}

function testerStatusLabel(row: TesterRow): string {
  if (row.power_state === 'error') return 'error';
  if (row.allocation === 'locked') return 'busy';
  if (row.allocation === 'upgrading') return 'upgrading';
  if (row.power_state === 'running' && row.allocation === 'idle') return 'idle';
  return row.power_state;
}

// ── Component ───────────────────────────────────────────────────────────

export function NewRunPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Run');

  // Step tracking
  const [step, setStep] = useState<Step>(0);

  // ── Step 0: Target type ──────────────────────────────────────────────
  const [endpointKind, setEndpointKind] = useState<EndpointKind>('runtime');
  // Network target fields
  const [host, setHost] = useState('');
  const [port, setPort] = useState('');
  // Proxy target fields
  const [proxyEndpointId, setProxyEndpointId] = useState('');
  const [proxySubType, setProxySubType] = useState<'existing' | 'create'>('existing');
  const [newTargetAccountId, setNewTargetAccountId] = useState('');
  const [newTargetRegion, setNewTargetRegion] = useState('');
  const [newTargetOs, setNewTargetOs] = useState<'linux' | 'windows'>('linux');
  const [newTargetHttpStack, setNewTargetHttpStack] = useState('nginx');
  const [newTargetEphemeral, setNewTargetEphemeral] = useState(true);
  // Runtime template
  const [runtimeTemplate, setRuntimeTemplate] = useState<string | null>(null);

  // ── Step 1: Testbeds (ported from AppBenchmarkWizardPage) ────────────
  const [testbedKey, setTestbedKey] = useState(0);
  const [testbeds, setTestbeds] = useState<TestbedState[]>([]);
  const [proxyWarning, setProxyWarning] = useState(false);

  // Runner selection (inline in testbed step, not a separate step)
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [testersLoading, setTestersLoading] = useState(false);

  // ── Step 2: Languages (for runtime only) ─────────────────────────────
  const [selectedLangs, setSelectedLangs] = useState<Set<string>>(new Set(['nginx']));

  // ── Step 3: Workload ─────────────────────────────────────────────────
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http2']));
  const [runs, setRuns] = useState(10);
  const [concurrency, setConcurrency] = useState(1);
  const [timeoutMs, setTimeoutMs] = useState(5000);
  const [selectedPayloads, setSelectedPayloads] = useState<Set<string>>(new Set());
  const [insecure, setInsecure] = useState(false);
  const [connectionReuse, setConnectionReuse] = useState(true);
  const [captureMode, setCaptureMode] = useState<'none' | 'tester' | 'endpoint' | 'both'>('none');

  // ── Step 4: Methodology ──────────────────────────────────────────────
  const [benchmarkMode, setBenchmarkMode] = useState(false);
  const [methodology, setMethodology] = useState<Methodology>(DEFAULT_METHODOLOGY);
  const [methodPreset, setMethodPreset] = useState<string>('standard');
  const [showAdvancedMethodology, setShowAdvancedMethodology] = useState(false);

  // ── Step 5: Review & Launch ──────────────────────────────────────────
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [submitting, setSubmitting] = useState(false);

  // Shared data
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [deploymentsLoading, setDeploymentsLoading] = useState(false);
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  const [cloudAccountsLoading, setCloudAccountsLoading] = useState(false);

  // ── Data loading ─────────────────────────────────────────────────────

  useEffect(() => {
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});

    setTestersLoading(true);
    testersApi.listTesters(projectId)
      .then(rows => setTesters(rows))
      .catch(() => setTesters([]))
      .finally(() => setTestersLoading(false));

    setDeploymentsLoading(true);
    api.getDeployments(projectId, { limit: 50 })
      .then(deps => setDeployments(deps))
      .catch(() => setDeployments([]))
      .finally(() => setDeploymentsLoading(false));

    setCloudAccountsLoading(true);
    api.getCloudAccounts(projectId)
      .then(accts => {
        const list = Array.isArray(accts) ? accts : [];
        list.sort((a, b) => {
          if (a.status === 'active' && b.status !== 'active') return -1;
          if (a.status !== 'active' && b.status === 'active') return 1;
          return a.provider.localeCompare(b.provider) || a.name.localeCompare(b.name);
        });
        setCloudAccounts(list);
        if (list.length > 0) {
          setNewTargetAccountId(list[0].account_id);
        }
      })
      .catch(() => setCloudAccounts([]))
      .finally(() => setCloudAccountsLoading(false));
  }, [projectId]);

  // ── Testbed helpers (ported from AppBenchmarkWizardPage) ──────────────

  const addTestbed = () => {
    const k = testbedKey;
    setTestbedKey(k + 1);
    setTestbeds(prev => [...prev, makeTestbed(k)]);
  };

  const removeTestbed = (key: number) => {
    setTestbeds(prev => prev.filter(c => c.key !== key));
  };

  const updateTestbed = (key: number, patch: Partial<TestbedState>) => {
    setTestbeds(prev => prev.map(c => {
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
    }));
  };

  // ── Language helpers ──────────────────────────────────────────────────

  const toggleLang = useCallback((id: string) => {
    setSelectedLangs(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const setLangShortcut = (ids: string[]) => {
    setSelectedLangs(new Set(ids));
  };

  // ── Methodology helpers ──────────────────────────────────────────────

  const applyMethodPreset = (presetId: string) => {
    const p = METHODOLOGY_PRESETS.find(m => m.id === presetId);
    if (!p) return;
    setMethodPreset(presetId);
    setMethodology(m => ({
      ...m,
      warmup_runs: p.warmup,
      measured_runs: p.measured,
      target_error_pct: p.targetError ?? m.target_error_pct,
    }));
  };

  // ── Template application ─────────────────────────────────────────────

  const applyTemplate = useCallback((tmpl: RuntimeTemplate) => {
    setRuntimeTemplate(tmpl.id);
    setSelectedLangs(new Set(tmpl.defaultLanguages));
    setSelectedModes(new Set(tmpl.defaultModes));

    // Pre-fill testbeds
    const newTestbeds: TestbedState[] = [];
    if (tmpl.id !== 'custom' && tmpl.defaultTestbedCount > 0) {
      const k = testbedKey;
      setTestbedKey(k + 1);
      newTestbeds.push(makeTestbed(k, 'Azure', tmpl.defaultOs ?? 'linux', tmpl.defaultProxies));
    }
    setTestbeds(newTestbeds);

    // Methodology
    if (tmpl.methodology !== 'quick') {
      setBenchmarkMode(true);
      applyMethodPreset(tmpl.methodology);
    }

    setProxyWarning(false);
    setStep(1);
  }, [testbedKey]);

  // ── Mode helpers ─────────────────────────────────────────────────────

  const handleModeToggle = useCallback((id: string) => {
    setSelectedModes(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const handleGroupToggle = useCallback((ids: string[], allSelected: boolean) => {
    setSelectedModes(prev => {
      const next = new Set(prev);
      for (const id of ids) {
        if (allSelected) next.delete(id);
        else next.add(id);
      }
      return next;
    });
  }, []);

  const hasThroughput = [...selectedModes].some(m => (THROUGHPUT_IDS as readonly string[]).includes(m));

  const handlePayloadToggle = useCallback((value: string) => {
    setSelectedPayloads(prev => {
      const next = new Set(prev);
      if (next.has(value)) next.delete(value);
      else next.add(value);
      return next;
    });
  }, []);

  // ── Derived state ────────────────────────────────────────────────────

  const selectedDeployment = useMemo(
    () => deployments.find(d => d.deployment_id === proxyEndpointId),
    [deployments, proxyEndpointId],
  );

  const selectedRunner = useMemo(
    () => runnerMode === 'specific' ? testers.find(t => t.tester_id === selectedTesterId) ?? null : null,
    [runnerMode, selectedTesterId, testers],
  );

  const runnerStats = useMemo(() => {
    const online = testers.filter(t => t.power_state === 'running');
    const busy = online.filter(t => t.allocation === 'locked');
    const idle = online.filter(t => t.allocation === 'idle');
    return { online: online.length, busy: busy.length, idle: idle.length };
  }, [testers]);

  const totalProxies = new Set(testbeds.flatMap(tb => tb.proxies)).size;
  const isRuntimeMode = endpointKind === 'runtime';

  // Determine effective step labels based on target type
  const effectiveSteps = useMemo((): string[] => {
    if (isRuntimeMode) {
      return ['Template', 'Testbeds', 'Languages', 'Workload', 'Methodology', 'Review'];
    }
    // Network/proxy: skip Template and Languages steps
    return ['Target', 'Testbeds', 'Workload', 'Methodology', 'Review'];
  }, [isRuntimeMode]);

  // ── Navigation ───────────────────────────────────────────────────────

  const canNext = useMemo(() => {
    if (step === 0) {
      if (isRuntimeMode) return runtimeTemplate !== null;
      if (endpointKind === 'network') return host.trim().length > 0;
      if (endpointKind === 'proxy') {
        if (proxySubType === 'existing') return proxyEndpointId.trim().length > 0;
        return newTargetAccountId !== '' && newTargetRegion !== '';
      }
      return true;
    }
    if (step === 1) return testbeds.length > 0 && testbeds.every(c => c.proxies.length > 0);
    if (step === 2) return !isRuntimeMode || selectedLangs.size > 0;
    if (step === 3) return selectedModes.size > 0;
    if (step === 4) return true;
    if (step === 5) return configName.trim().length > 0;
    return true;
  }, [step, isRuntimeMode, runtimeTemplate, endpointKind, host, proxyEndpointId, proxySubType, newTargetAccountId, newTargetRegion, testbeds, selectedLangs.size, selectedModes.size, configName]);

  const goNext = () => {
    if (!canNext) return;
    // Validate proxies before advancing past Testbeds step
    if (step === 1) {
      const missingProxies = testbeds.some(tb => tb.proxies.length === 0);
      if (missingProxies) {
        setProxyWarning(true);
        return;
      }
      setProxyWarning(false);
    }
    // Auto-switch single Linux testbed to Windows when .NET 4.8 is selected
    if (step === 2 && requiresWindows(selectedLangs)) {
      setTestbeds(prev => prev.map(tb => {
        if (tb.os === 'linux' && prev.length === 1) {
          const validProxies = (WINDOWS_PROXIES as readonly string[]);
          return { ...tb, os: 'windows' as const, proxies: tb.proxies.filter(p => validProxies.includes(p)) };
        }
        return tb;
      }));
    }
    // Skip languages step for non-runtime targets
    if (step === 1 && !isRuntimeMode) {
      setStep(3);
      return;
    }
    if (step < 5) {
      setStep((step + 1) as Step);
    }
  };

  const goBack = () => {
    if (step === 0) return;
    // Skip languages step going back for non-runtime
    if (step === 3 && !isRuntimeMode) {
      setStep(1);
      return;
    }
    setStep((step - 1) as Step);
  };

  // ── Submit ───────────────────────────────────────────────────────────

  const buildEndpoint = (): EndpointRef => {
    switch (endpointKind) {
      case 'network':
        return { kind: 'network', host, ...(port ? { port: Number(port) } : {}) };
      case 'proxy':
        return { kind: 'proxy', proxy_endpoint_id: proxyEndpointId };
      case 'runtime':
        return { kind: 'runtime', runtime_id: runtimeTemplate ?? '', language: [...selectedLangs][0] ?? '' };
    }
  };

  /** Build cells for the comparison group from the cartesian product of languages x testbeds. */
  const buildComparisonCells = (): ComparisonCell[] => {
    const cells: ComparisonCell[] = [];
    const langs = isRuntimeMode && selectedLangs.size > 0 ? [...selectedLangs] : [''];

    for (const lang of langs) {
      for (const tb of testbeds) {
        const label = [
          lang,
          `${tb.cloud}/${tb.region}`,
          tb.os,
        ].filter(Boolean).join(' @ ');
        cells.push({
          label,
          endpoint: buildEndpoint(),
          ...(selectedTesterId ? { runner_id: selectedTesterId } : {}),
        });
      }
    }
    return cells;
  };

  const isMatrixRun = (isRuntimeMode && selectedLangs.size > 1) || testbeds.length > 1;

  const handleSubmit = async (launchNow: boolean) => {
    setSubmitting(true);
    try {
      const sizeMap: Record<string, number> = { '64k': 65536, '1m': 1048576, '16m': 16777216 };
      const payloadSizes = [...selectedPayloads].map(s => sizeMap[s]).filter(Boolean);
      const captureModeMap: Record<string, string> = { none: 'metrics-only', tester: 'headers-only', endpoint: 'headers-only', both: 'full' };

      const workload: Workload = {
        modes: [...selectedModes],
        runs,
        concurrency,
        timeout_ms: timeoutMs,
        payload_sizes: payloadSizes,
        capture_mode: (captureModeMap[captureMode] ?? 'headers-only') as Workload['capture_mode'],
        insecure: insecure || undefined,
        connection_reuse: connectionReuse || undefined,
      };

      if (isMatrixRun && launchNow) {
        const cells = buildComparisonCells();
        const body: ComparisonGroupCreate = {
          name: configName,
          base_workload: workload,
          ...(benchmarkMode ? { methodology } : {}),
          cells,
        };
        const group = await api.createComparisonGroup(projectId, body);
        addToast('success', `Comparison group launched -- ${cells.length} runs`);
        navigate(`/projects/${projectId}/runs?comparison_group=${group.id}`);
        return;
      }

      const config: TestConfigCreate = {
        name: configName,
        endpoint: buildEndpoint(),
        workload,
        ...(benchmarkMode ? { methodology } : {}),
      };

      const created = await api.createTestConfig(projectId, config);

      if (addSchedule) {
        await api.createTestSchedule(projectId, {
          test_config_id: created.id,
          cron_expr: cronExpr,
        });
      }

      if (launchNow) {
        const run = await api.launchTestConfig(created.id);
        addToast('success', `Run ${run.id.slice(0, 8)} launched`);
        navigate(`/projects/${projectId}/runs/${run.id}`);
      } else {
        addToast('success', `Config "${configName}" saved`);
        navigate(`/projects/${projectId}/runs`);
      }
    } catch (e) {
      addToast('error', `Failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  // ── Render ────────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'New Run' }]} />

      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">New Run</h2>
        <p className="text-xs text-gray-500 mt-1">
          Configure workload tests against your deployed targets.
        </p>
      </div>

      {/* ── Stepper (ported from production wizard) ── */}
      <div className="flex items-center gap-0.5 mb-8 font-mono text-xs">
        {effectiveSteps.map((label, i) => {
          // Map display index to actual step value
          let actualStep: Step;
          if (isRuntimeMode) {
            actualStep = i as Step;
          } else {
            // Non-runtime: Target(0), Testbeds(1), Workload(3), Methodology(4), Review(5)
            const nonRuntimeMap: Step[] = [0, 1, 3, 4, 5];
            actualStep = nonRuntimeMap[i];
          }
          const isCurrent = actualStep === step;
          const isPast = actualStep < step;
          return (
            <button
              key={label}
              onClick={() => { if (isPast) setStep(actualStep); }}
              disabled={!isPast && !isCurrent}
              className={`px-2 py-1 transition-colors ${
                isCurrent
                  ? 'text-cyan-300'
                  : isPast
                    ? 'text-gray-500 hover:text-gray-300 cursor-pointer'
                    : 'text-gray-700 cursor-not-allowed'
              }`}
            >
              {i + 1}. {label}
            </button>
          );
        })}
      </div>

      {/* ── Step 0: Target Type / Template ── */}
      {step === 0 && (
        <div>
          {/* Target type selector */}
          <div className="mb-6">
            <label className="text-xs text-gray-500 mb-2 block">Target Type</label>
            <div className="flex">
              {([
                { kind: 'runtime' as const, label: 'Runtime (stack)', desc: 'Compare language stacks' },
                { kind: 'proxy' as const, label: 'Proxy (target)', desc: 'Use a deployed endpoint' },
              ]).map(({ kind, label }) => (
                <button
                  key={kind}
                  onClick={() => setEndpointKind(kind)}
                  className={`px-3 py-1.5 text-xs font-mono border transition-colors ${
                    endpointKind === kind
                      ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300 z-10'
                      : 'border-gray-700 text-gray-500 hover:text-gray-300'
                  } ${kind === 'runtime' ? '' : '-ml-px'}`}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>

          {/* Runtime: template gallery */}
          {endpointKind === 'runtime' && (
            <div>
              <h3 className="text-sm font-semibold text-gray-200 mb-4">Choose a template</h3>
              <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
                {RUNTIME_TEMPLATES.map(tmpl => (
                  <button
                    key={tmpl.id}
                    onClick={() => applyTemplate(tmpl)}
                    className={`text-left border px-3 py-2.5 transition-colors ${
                      runtimeTemplate === tmpl.id
                        ? 'border-cyan-500/50 bg-cyan-500/5'
                        : 'border-gray-800 hover:border-gray-600'
                    }`}
                  >
                    <div className="text-sm font-medium text-gray-100">{tmpl.name}</div>
                    <div className="text-[11px] text-gray-500 mt-0.5">{tmpl.description}</div>
                    {tmpl.defaultTestbedCount > 0 && (
                      <div className="text-[10px] font-mono text-gray-600 mt-1.5">
                        {tmpl.defaultTestbedCount} testbed / {tmpl.defaultLanguages.length} lang
                      </div>
                    )}
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* Network: host + port */}
          {endpointKind === 'network' && (
            <div className="space-y-3">
              <div>
                <label htmlFor="host" className="text-xs text-gray-500 mb-1 block">Host</label>
                <input
                  id="host"
                  type="text"
                  value={host}
                  onChange={e => setHost(e.target.value)}
                  placeholder="e.g. www.cloudflare.com"
                  className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                />
              </div>
              <div>
                <label htmlFor="port" className="text-xs text-gray-500 mb-1 block">Port (optional)</label>
                <input
                  id="port"
                  type="number"
                  value={port}
                  onChange={e => setPort(e.target.value)}
                  placeholder="443"
                  className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-32 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                />
              </div>
            </div>
          )}

          {/* Proxy: existing or create */}
          {endpointKind === 'proxy' && (
            <div className="space-y-3">
              <div className="flex gap-1 bg-gray-900 p-0.5 w-fit">
                {(['existing', 'create'] as const).map(sub => (
                  <button
                    key={sub}
                    onClick={() => setProxySubType(sub)}
                    className={`px-3 py-1 text-xs transition-colors ${
                      proxySubType === sub
                        ? 'bg-gray-700 text-gray-100'
                        : 'text-gray-500 hover:text-gray-300'
                    }`}
                  >
                    {sub === 'existing' ? 'Use existing target' : 'Create new target'}
                  </button>
                ))}
              </div>

              {proxySubType === 'existing' && (
                <>
                  {deploymentsLoading && (
                    <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading targets...</p>
                  )}
                  {!deploymentsLoading && deployments.length === 0 && (
                    <div className="border border-dashed border-gray-800 p-4">
                      <p className="text-sm text-gray-300 mb-1">No targets deployed yet.</p>
                      <button type="button" onClick={() => setProxySubType('create')} className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors">
                        Create new target
                      </button>
                    </div>
                  )}
                  {!deploymentsLoading && deployments.length > 0 && (
                    <div className="space-y-2" role="radiogroup" aria-label="Deployed targets">
                      {deployments.map(dep => {
                        const checked = proxyEndpointId === dep.deployment_id;
                        const ips = dep.endpoint_ips ?? [];
                        const firstEndpoint = dep.config?.endpoints?.[0];
                        return (
                          <label
                            key={dep.deployment_id}
                            className={`block border p-3 cursor-pointer transition-colors ${
                              checked ? 'border-cyan-500/50 bg-cyan-500/5' : 'border-gray-800 hover:border-gray-600'
                            }`}
                          >
                            <div className="flex items-center gap-3">
                              <input type="radio" name="proxy-endpoint" value={dep.deployment_id} checked={checked} onChange={() => setProxyEndpointId(dep.deployment_id)} className="accent-cyan-400" />
                              <div className="flex-1 min-w-0">
                                <span className="text-sm font-medium text-gray-100">{dep.name}</span>
                                <div className="flex items-center gap-2 mt-0.5">
                                  {firstEndpoint?.provider && <span className="text-[10px] font-mono text-gray-500">{firstEndpoint.provider}</span>}
                                  {firstEndpoint?.region && <span className="text-[10px] font-mono text-gray-500">{firstEndpoint.region}</span>}
                                  {ips.length > 0 && <span className="text-[10px] font-mono text-gray-600">{ips[0]}</span>}
                                </div>
                              </div>
                              <span className={`text-[10px] font-mono px-1.5 py-0.5 border rounded ${deploymentStatusClass(dep.status)}`}>{dep.status}</span>
                            </div>
                          </label>
                        );
                      })}
                    </div>
                  )}
                </>
              )}

              {proxySubType === 'create' && (
                <div className="border border-gray-800 p-4 space-y-3">
                  <div>
                    <label htmlFor="new-target-cloud" className="block text-xs text-gray-400 mb-1">Cloud Account</label>
                    {cloudAccountsLoading ? (
                      <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading accounts...</p>
                    ) : (
                      <select
                        id="new-target-cloud"
                        value={newTargetAccountId}
                        onChange={e => {
                          setNewTargetAccountId(e.target.value);
                          const acct = cloudAccounts.find(a => a.account_id === e.target.value);
                          if (acct) {
                            const regions = DEPLOY_REGIONS[acct.provider] ?? [];
                            setNewTargetRegion(regions[0] ?? '');
                          }
                        }}
                        className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        {cloudAccounts.length === 0 && <option disabled value="">No cloud accounts</option>}
                        {cloudAccounts.map(a => (
                          <option key={a.account_id} value={a.account_id}>
                            {a.provider.toUpperCase()} -- {a.name}
                          </option>
                        ))}
                      </select>
                    )}
                  </div>
                  <div className="grid grid-cols-2 gap-3">
                    <div>
                      <label htmlFor="new-target-region" className="block text-xs text-gray-400 mb-1">Region</label>
                      <select
                        id="new-target-region"
                        value={newTargetRegion}
                        onChange={e => setNewTargetRegion(e.target.value)}
                        className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        {(DEPLOY_REGIONS[cloudAccounts.find(a => a.account_id === newTargetAccountId)?.provider ?? 'azure'] ?? []).map(r => (
                          <option key={r} value={r}>{r}</option>
                        ))}
                      </select>
                    </div>
                    <div>
                      <label htmlFor="new-target-os" className="block text-xs text-gray-400 mb-1">OS</label>
                      <select
                        id="new-target-os"
                        value={newTargetOs}
                        onChange={e => {
                          const os = e.target.value as 'linux' | 'windows';
                          setNewTargetOs(os);
                          setNewTargetHttpStack(os === 'windows' ? 'iis' : 'nginx');
                        }}
                        className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        <option value="linux">Linux (Ubuntu)</option>
                        <option value="windows">Windows Server</option>
                      </select>
                    </div>
                  </div>
                  <div>
                    <label htmlFor="new-target-stack" className="block text-xs text-gray-400 mb-1">HTTP Stack</label>
                    <select
                      id="new-target-stack"
                      value={newTargetHttpStack}
                      onChange={e => setNewTargetHttpStack(e.target.value)}
                      className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                    >
                      {HTTP_STACKS.map(s => <option key={s} value={s}>{s.toUpperCase()}</option>)}
                    </select>
                  </div>
                  <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                    <input type="checkbox" checked={newTargetEphemeral} onChange={e => setNewTargetEphemeral(e.target.checked)} className="accent-cyan-500" />
                    Destroy after run
                  </label>
                </div>
              )}
            </div>
          )}

          {/* Next button for non-runtime targets (runtime uses template click) */}
          {endpointKind !== 'runtime' && (
            <div className="mt-6">
              <button
                onClick={() => { setStep(1); if (testbeds.length === 0) addTestbed(); }}
                disabled={!canNext}
                className="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 disabled:bg-gray-800 disabled:text-gray-600 text-white text-xs font-medium transition-colors"
              >
                Next
              </button>
            </div>
          )}
        </div>
      )}

      {/* ── Step 1: Testbeds (ported from AppBenchmarkWizardPage) ── */}
      {step === 1 && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-sm font-semibold text-gray-200">Configure Testbeds</h3>
            <button
              onClick={addTestbed}
              className="px-3 py-1.5 border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
            >
              + Add Testbed
            </button>
          </div>

          {testbeds.length === 0 && (
            <div className="border border-dashed border-gray-800 p-4">
              <p className="text-xs text-gray-500 mb-3">No testbeds configured. Add one to define where tests run.</p>
              <div className="flex flex-wrap gap-2">
                {[
                  { cloud: 'Azure', os: 'linux' as const },
                  { cloud: 'Azure', os: 'windows' as const },
                  { cloud: 'AWS', os: 'linux' as const },
                  { cloud: 'GCP', os: 'linux' as const },
                ].map(({ cloud, os }) => (
                  <button
                    key={`${cloud}-${os}`}
                    onClick={() => { const k = testbedKey; setTestbedKey(k + 1); setTestbeds([makeTestbed(k, cloud, os)]); }}
                    className="px-3 py-1.5 text-xs font-mono border border-gray-700 text-gray-300 hover:border-cyan-500 hover:text-cyan-300 transition-colors"
                  >
                    + {cloud} / {os === 'linux' ? 'Linux' : 'Windows'}
                  </button>
                ))}
              </div>
            </div>
          )}

          {proxyWarning && (
            <div className="mb-3 border border-yellow-500/30 bg-yellow-500/5 p-3">
              <p className="text-xs text-yellow-300">
                Every testbed must have at least one reverse proxy selected before proceeding.
              </p>
            </div>
          )}

          <div className="space-y-2">
            {testbeds.map((testbed, idx) => (
              <div key={testbed.key} className="border border-gray-800 p-3">
                {/* Row 1: Cloud, OS, Region, Topology, Size, Remove */}
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-[10px] font-mono text-gray-600 w-3">{idx + 1}</span>

                  {/* Cloud toggle buttons */}
                  <div className="flex">
                    {CLOUDS.map(c => (
                      <button
                        key={c}
                        onClick={() => updateTestbed(testbed.key, { cloud: c })}
                        className={`px-2.5 py-1 text-xs font-mono border transition-colors ${
                          testbed.cloud === c
                            ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300 z-10'
                            : 'border-gray-700 text-gray-500 hover:text-gray-300'
                        } ${c === 'Azure' ? '' : '-ml-px'}`}
                      >
                        {c}
                      </button>
                    ))}
                  </div>

                  {/* OS toggle buttons */}
                  <div className="flex">
                    {(['linux', 'windows'] as const).map(os => (
                      <button
                        key={os}
                        onClick={() => updateTestbed(testbed.key, { os })}
                        className={`px-2.5 py-1 text-xs font-mono border transition-colors ${
                          testbed.os === os
                            ? os === 'linux'
                              ? 'bg-green-500/10 border-green-500/40 text-green-300 z-10'
                              : 'bg-blue-500/10 border-blue-500/40 text-blue-300 z-10'
                            : 'border-gray-700 text-gray-500 hover:text-gray-300'
                        } ${os === 'linux' ? '' : '-ml-px'}`}
                      >
                        {os === 'linux' ? 'Linux' : 'Windows'}
                      </button>
                    ))}
                  </div>

                  {/* Region dropdown */}
                  <select
                    value={testbed.region}
                    onChange={e => updateTestbed(testbed.key, { region: e.target.value })}
                    className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
                  >
                    {(REGIONS[testbed.cloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                  </select>

                  {/* Topology dropdown */}
                  <select
                    value={testbed.topology}
                    onChange={e => updateTestbed(testbed.key, { topology: e.target.value })}
                    className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs text-gray-500 focus:outline-none focus:border-cyan-500"
                  >
                    {TOPOLOGIES.map(t => <option key={t} value={t}>{t}</option>)}
                  </select>

                  {/* VM Size dropdown */}
                  <select
                    value={testbed.vmSize}
                    onChange={e => updateTestbed(testbed.key, { vmSize: e.target.value })}
                    className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs text-gray-500 focus:outline-none focus:border-cyan-500"
                  >
                    {VM_SIZES.map(s => <option key={s} value={s}>{s}</option>)}
                  </select>

                  <button
                    onClick={() => removeTestbed(testbed.key)}
                    className="text-[11px] text-gray-600 hover:text-red-400 transition-colors ml-auto"
                  >
                    remove
                  </button>
                </div>

                {/* Existing VM checkbox */}
                <div className="mt-2">
                  <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={testbed.existingVm}
                      onChange={e => updateTestbed(testbed.key, { existingVm: e.target.checked })}
                      className="accent-cyan-400"
                    />
                    Use existing VM
                  </label>
                  {testbed.existingVm && (
                    <input
                      type="text"
                      value={testbed.existingVmId}
                      onChange={e => updateTestbed(testbed.key, { existingVmId: e.target.value })}
                      placeholder="VM ID or IP from catalog"
                      className="mt-1 bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                    />
                  )}
                </div>

                {/* Reverse Proxies toggle buttons */}
                <div className="mt-3">
                  <label className="block text-xs text-gray-500 mb-1.5">Reverse Proxies</label>
                  <div className="flex flex-wrap gap-2">
                    {(testbed.os === 'windows' ? WINDOWS_PROXIES : LINUX_PROXIES).map(p => (
                      <button
                        key={p}
                        type="button"
                        onClick={() => {
                          const current = testbed.proxies;
                          const next = current.includes(p)
                            ? current.filter(x => x !== p)
                            : [...current, p];
                          updateTestbed(testbed.key, { proxies: next });
                        }}
                        className={`px-2.5 py-1 text-xs border transition-colors ${
                          testbed.proxies.includes(p)
                            ? 'bg-cyan-900/40 border-cyan-700 text-cyan-300'
                            : 'border-gray-700 text-gray-400 hover:border-gray-600'
                        }`}
                      >
                        {PROXY_LABELS[p]}
                      </button>
                    ))}
                  </div>
                  {testbed.proxies.length === 0 && (
                    <p className="text-xs text-yellow-500 mt-1">At least one proxy is required</p>
                  )}
                </div>

                {/* Tester VM type toggle buttons */}
                <div className="mt-3">
                  <label className="block text-xs text-gray-500 mb-1.5">Runner VM</label>
                  <div className="flex gap-2">
                    {TESTER_OS_OPTIONS.map(opt => (
                      <button
                        key={opt.id}
                        type="button"
                        onClick={() => updateTestbed(testbed.key, { testerOs: opt.id })}
                        className={`px-2.5 py-1 text-xs border transition-colors ${
                          testbed.testerOs === opt.id
                            ? 'bg-cyan-900/40 border-cyan-700 text-cyan-300'
                            : 'border-gray-700 text-gray-400 hover:border-gray-600'
                        }`}
                      >
                        {opt.label}
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            ))}
          </div>

          {/* Runner selection (inline, not a separate step) */}
          {testbeds.length > 0 && (
            <div className="mt-6 border border-gray-800 p-4">
              <div className="flex items-center justify-between mb-3">
                <h4 className="text-xs font-semibold text-gray-300 uppercase tracking-wider">Runner Assignment</h4>
                {!testersLoading && (
                  <span className="text-[10px] font-mono text-gray-600">
                    {runnerStats.idle} idle / {runnerStats.online} online
                  </span>
                )}
              </div>

              <div className="flex gap-2 mb-3">
                {(['auto', 'specific'] as const).map(mode => (
                  <button
                    key={mode}
                    onClick={() => { setRunnerMode(mode); if (mode === 'auto') setSelectedTesterId(null); }}
                    className={`px-2.5 py-1 text-xs border transition-colors ${
                      runnerMode === mode
                        ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300'
                        : 'border-gray-700 text-gray-500 hover:text-gray-300'
                    }`}
                  >
                    {mode === 'auto' ? 'Auto-pick' : 'Pick specific'}
                  </button>
                ))}
              </div>

              {runnerMode === 'auto' && (
                <p className="text-xs text-gray-500">First available idle runner will be assigned.</p>
              )}

              {runnerMode === 'specific' && (
                <div className="space-y-1">
                  {testersLoading && <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading runners...</p>}
                  {!testersLoading && testers.length === 0 && <p className="text-xs text-gray-500">No runners available.</p>}
                  {!testersLoading && testers.length > 0 && testers.map(row => {
                    const isOnline = row.power_state === 'running';
                    const isIdle = isOnline && row.allocation === 'idle';
                    const isBusy = isOnline && row.allocation === 'locked';
                    const checked = selectedTesterId === row.tester_id;
                    return (
                      <label
                        key={row.tester_id}
                        className={`block border p-2.5 transition-colors ${
                          !isOnline ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'
                        } ${
                          checked ? 'border-cyan-500/50 bg-cyan-500/5' : 'border-gray-800 hover:border-gray-600'
                        }`}
                      >
                        <div className="flex items-center gap-3">
                          <input
                            type="radio"
                            name="runner-specific"
                            value={row.tester_id}
                            checked={checked}
                            disabled={!isOnline}
                            onChange={() => setSelectedTesterId(row.tester_id)}
                            className="accent-cyan-400"
                          />
                          <span className={`w-1.5 h-1.5 rounded-full ${isIdle ? 'bg-green-400' : isBusy ? 'bg-yellow-400' : 'bg-gray-600'}`} />
                          <span className="text-sm font-medium text-gray-100 flex-1">{row.name}</span>
                          <span className="text-[10px] font-mono text-gray-500">{row.cloud} / {row.region}</span>
                          <span className={`text-[10px] font-mono px-1.5 py-0.5 border rounded ${testerStatusClass(row)}`}>
                            {testerStatusLabel(row)}
                          </span>
                        </div>
                      </label>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {/* ── Step 2: Languages (runtime only) ── */}
      {step === 2 && isRuntimeMode && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-sm font-semibold text-gray-200">Select Languages</h3>
            <div className="flex items-center gap-2">
              <button
                onClick={() => setLangShortcut(ALL_LANGUAGE_IDS)}
                className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
              >
                Select All
              </button>
              <button
                onClick={() => setLangShortcut(TOP_5_IDS)}
                className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
              >
                Top 5
              </button>
              <button
                onClick={() => setLangShortcut(SYSTEMS_IDS)}
                className="px-2 py-1 border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
              >
                Systems Only
              </button>
            </div>
          </div>

          <div className="space-y-5">
            {LANGUAGE_GROUPS.map(group => (
              <div key={group.label}>
                <div className="text-[10px] uppercase tracking-wider font-mono text-gray-600 mb-2">{group.label}</div>
                <div className="flex flex-wrap gap-1.5">
                  {group.entries.map(entry => {
                    const checked = selectedLangs.has(entry.id);
                    const isNginx = entry.id === 'nginx';
                    return (
                      <label
                        key={entry.id}
                        className={`flex items-center gap-2 px-3 py-2 border cursor-pointer transition-colors text-xs ${
                          checked
                            ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-200'
                            : 'border-gray-800 text-gray-400 hover:border-gray-600'
                        }`}
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleLang(entry.id)}
                          disabled={isNginx}
                          className="accent-cyan-400"
                        />
                        <span>{entry.label}</span>
                        {isNginx && (
                          <span className="text-[10px] uppercase tracking-wider text-cyan-500/70 border border-cyan-500/30 px-1 py-0.5">
                            baseline
                          </span>
                        )}
                      </label>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>

          <p className="text-xs text-gray-600 mt-4">
            {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''} selected.
            {selectedLangs.has('nginx') && ' nginx is included as the static baseline.'}
          </p>

          {requiresWindows(selectedLangs) && testbeds.length === 1 && testbeds[0].os === 'linux' && (
            <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 p-3">
              <p className="text-xs text-yellow-300">
                C# .NET 4.8 requires Windows Server. When you proceed, your testbed will be switched to Windows automatically.
              </p>
            </div>
          )}

          {requiresWindows(selectedLangs) && testbeds.length > 1 && testbeds.some(tb => tb.os === 'linux') && (
            <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 p-3">
              <p className="text-xs text-yellow-300">
                C# .NET 4.8 requires Windows Server. It will only run on testbeds configured with Windows OS.
                Linux testbeds will skip .NET 4.8 automatically.
              </p>
            </div>
          )}
        </div>
      )}

      {/* ── Step 3: Workload ── */}
      {step === 3 && (
        <div className="space-y-4">
          <h3 className="text-sm font-semibold text-gray-200 mb-2">Workload Configuration</h3>

          {modeGroups.length > 0 && (
            <div>
              <label className="text-xs text-gray-500 mb-2 block">Modes</label>
              <ModeSelector
                modeGroups={modeGroups}
                selectedModes={selectedModes}
                onToggle={handleModeToggle}
                onToggleGroup={handleGroupToggle}
              />
            </div>
          )}

          <div className="grid grid-cols-3 gap-4">
            <div>
              <label htmlFor="runs" className="text-xs text-gray-500 mb-1 block">Runs</label>
              <input
                id="runs"
                type="number"
                min={1}
                value={runs}
                onChange={e => setRuns(Number(e.target.value))}
                className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div>
              <label htmlFor="concurrency" className="text-xs text-gray-500 mb-1 block">Concurrency</label>
              <input
                id="concurrency"
                type="number"
                min={1}
                value={concurrency}
                onChange={e => setConcurrency(Number(e.target.value))}
                className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div>
              <label htmlFor="timeout" className="text-xs text-gray-500 mb-1 block">Timeout (ms)</label>
              <input
                id="timeout"
                type="number"
                min={100}
                value={timeoutMs}
                onChange={e => setTimeoutMs(Number(e.target.value))}
                className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
              />
            </div>
          </div>

          {hasThroughput && (
            <div>
              <label className="text-xs text-gray-500 mb-2 block">Payload Sizes</label>
              <PayloadSelector selected={selectedPayloads} onToggle={handlePayloadToggle} />
            </div>
          )}

          {/* Advanced options */}
          <details className="border border-gray-800">
            <summary className="px-4 py-3 text-xs font-semibold text-gray-400 uppercase tracking-wider cursor-pointer hover:text-gray-300 transition-colors select-none">
              Advanced
            </summary>
            <div className="px-4 pb-4 space-y-3">
              <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                <input type="checkbox" checked={insecure} onChange={e => setInsecure(e.target.checked)} className="accent-cyan-500" />
                Allow insecure HTTPS (skip TLS verification)
              </label>
              <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                <input type="checkbox" checked={connectionReuse} onChange={e => setConnectionReuse(e.target.checked)} className="accent-cyan-500" />
                Reuse connections (keep-alive)
              </label>
              <div>
                <label htmlFor="capture-mode" className="block text-xs text-gray-400 mb-1">Capture mode</label>
                <select
                  id="capture-mode"
                  value={captureMode}
                  onChange={e => setCaptureMode(e.target.value as typeof captureMode)}
                  className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                >
                  <option value="none">None</option>
                  <option value="tester">Tester-side</option>
                  <option value="endpoint">Endpoint-side</option>
                  <option value="both">Both</option>
                </select>
              </div>
            </div>
          </details>
        </div>
      )}

      {/* ── Step 4: Methodology ── */}
      {step === 4 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Methodology</h3>

          <label className="flex items-center gap-3 cursor-pointer mb-6">
            <input
              type="checkbox"
              checked={benchmarkMode}
              onChange={e => setBenchmarkMode(e.target.checked)}
              className="w-4 h-4 border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
            />
            <div>
              <span className="text-sm text-gray-200">Enable benchmark mode</span>
              <p className="text-xs text-gray-500">Adds warmup, measured iterations, quality gates, and generates a publishable artifact.</p>
            </div>
          </label>

          {benchmarkMode && (
            <>
              {/* Presets */}
              <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-6">
                {METHODOLOGY_PRESETS.map(p => (
                  <button
                    key={p.id}
                    onClick={() => applyMethodPreset(p.id)}
                    className={`text-left border p-4 transition-colors ${
                      methodPreset === p.id
                        ? 'border-cyan-500/50 bg-cyan-500/5'
                        : 'border-gray-800 hover:border-gray-600'
                    }`}
                  >
                    <h4 className="text-sm font-medium text-gray-100">{p.label}</h4>
                    <div className="text-xs text-gray-500 mt-2 space-y-1">
                      <div>{p.warmup} warmup runs</div>
                      <div>{p.measured} measured runs</div>
                      <div>{p.targetError != null ? `${p.targetError}% target error` : 'No error target'}</div>
                    </div>
                  </button>
                ))}
              </div>

              {/* Advanced toggle */}
              <button
                onClick={() => setShowAdvancedMethodology(!showAdvancedMethodology)}
                className="text-xs text-gray-400 hover:text-gray-200 transition-colors mb-4"
              >
                {showAdvancedMethodology ? 'Hide' : 'Show'} advanced options
              </button>

              {showAdvancedMethodology && (
                <div className="border border-gray-800 p-4 space-y-4">
                  <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
                    <label className="text-xs text-gray-500">
                      Warmup runs
                      <input
                        type="number"
                        value={methodology.warmup_runs}
                        onChange={e => { setMethodology(m => ({ ...m, warmup_runs: Number(e.target.value) })); setMethodPreset('custom'); }}
                        min={0}
                        className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      />
                    </label>
                    <label className="text-xs text-gray-500">
                      Measured runs
                      <input
                        type="number"
                        value={methodology.measured_runs}
                        onChange={e => { setMethodology(m => ({ ...m, measured_runs: Number(e.target.value) })); setMethodPreset('custom'); }}
                        min={1}
                        className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      />
                    </label>
                    <label className="text-xs text-gray-500">
                      Cooldown (ms)
                      <input
                        type="number"
                        value={methodology.cooldown_ms}
                        onChange={e => { setMethodology(m => ({ ...m, cooldown_ms: Number(e.target.value) })); setMethodPreset('custom'); }}
                        min={0}
                        className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      />
                    </label>
                    <label className="text-xs text-gray-500">
                      Target error %
                      <input
                        type="number"
                        value={methodology.target_error_pct}
                        onChange={e => { setMethodology(m => ({ ...m, target_error_pct: Number(e.target.value) })); setMethodPreset('custom'); }}
                        min={0}
                        step={0.5}
                        className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      />
                    </label>
                  </div>
                  <div className="text-xs text-gray-600 pt-2">
                    Quality gates: CV &lt; {methodology.quality_gates.max_cv_pct}%, min {methodology.quality_gates.min_samples} samples.
                    Publication gate: failure rate &lt; {methodology.publication_gates.max_failure_pct}%.
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      )}

      {/* ── Step 5: Review & Launch ── */}
      {step === 5 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Review & Launch</h3>

          {/* Config name */}
          <label className="text-xs text-gray-500 block mb-4">
            Configuration name
            <input
              type="text"
              value={configName}
              onChange={e => setConfigName(e.target.value)}
              placeholder={isRuntimeMode ? `Runtime benchmark ${new Date().toISOString().slice(0, 10)}` : 'e.g. cloudflare-http2-daily'}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </label>

          {/* Summary */}
          <div className="text-xs font-mono text-gray-400 mb-4">
            {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
            {isRuntimeMode && <> / {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''}</>}
            {' / '}{totalProxies} prox{totalProxies !== 1 ? 'ies' : 'y'}
            {' / '}{[...selectedModes].join(', ')}
          </div>

          {/* Target info */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Target</div>
            <div className="text-xs font-mono text-gray-400">
              {endpointKind === 'network' && `Network: ${host}${port ? `:${port}` : ''}`}
              {endpointKind === 'proxy' && proxySubType === 'existing' && selectedDeployment && `Proxy: ${selectedDeployment.name}`}
              {endpointKind === 'proxy' && proxySubType === 'create' && `Proxy: new (${newTargetOs}, ${newTargetHttpStack})`}
              {endpointKind === 'runtime' && `Runtime: ${runtimeTemplate}`}
            </div>
          </div>

          {/* Testbeds */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Testbeds</div>
            <div className="space-y-0.5">
              {testbeds.map((testbed, idx) => (
                <div key={testbed.key} className="flex items-center gap-2 text-xs font-mono py-1 border-b border-gray-800/50 last:border-0">
                  <span className="text-gray-500 w-4">{idx + 1}</span>
                  <span className="text-gray-200">{testbed.cloud}</span>
                  <span className="text-gray-500">/</span>
                  <span className="text-gray-300">{testbed.region}</span>
                  <span className={`text-[10px] px-1 ${testbed.os === 'windows' ? 'text-blue-400' : 'text-green-400'}`}>
                    {testbed.os === 'windows' ? 'win' : 'linux'}
                  </span>
                  <span className="text-gray-600">{testbed.vmSize}</span>
                  <span className="text-gray-700">{testbed.topology}</span>
                  <span className="text-cyan-500/70">{testbed.proxies.map(p => PROXY_LABELS[p] ?? p).join(', ')}</span>
                  <span className="text-gray-600">{TESTER_OS_OPTIONS.find(o => o.id === testbed.testerOs)?.label ?? testbed.testerOs}</span>
                </div>
              ))}
            </div>
          </div>

          {/* Runner */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Runner</div>
            <div className="text-xs font-mono text-gray-400">
              {runnerMode === 'auto' && 'Auto-pick (first available)'}
              {runnerMode === 'specific' && selectedRunner && `${selectedRunner.name} (${selectedRunner.cloud} / ${selectedRunner.region})`}
              {runnerMode === 'specific' && !selectedRunner && 'None selected'}
            </div>
          </div>

          {/* Languages (runtime only) */}
          {isRuntimeMode && selectedLangs.size > 0 && (
            <div className="mb-4">
              <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Languages</div>
              <div className="text-xs font-mono text-gray-400">
                {[...selectedLangs].sort().map(lang => {
                  const entry = LANGUAGE_GROUPS.flatMap(g => g.entries).find(e => e.id === lang);
                  return entry?.label ?? lang;
                }).join(', ')}
              </div>
            </div>
          )}

          {/* Methodology */}
          {benchmarkMode && (
            <div className="mb-4">
              <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Methodology</div>
              <div className="text-xs font-mono text-gray-400">
                {methodology.warmup_runs} warmup / {methodology.measured_runs} measured / {methodology.target_error_pct}% target error
              </div>
            </div>
          )}

          {/* Workload */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Workload</div>
            <div className="text-xs font-mono text-gray-400">
              {runs} runs x {concurrency} concurrency / {timeoutMs}ms timeout / {[...selectedModes].join(' ')}
            </div>
          </div>

          {/* Warnings */}
          {insecure && <p className="text-xs text-yellow-400 mb-2">Insecure mode (TLS verification disabled)</p>}
          {endpointKind === 'proxy' && proxySubType === 'create' && (
            <div className="bg-blue-500/10 border border-blue-500/30 p-2 text-xs text-blue-300 mb-4">
              Target will be deployed first (~2 min).
              {newTargetEphemeral && ' Auto-teardown after run.'}
            </div>
          )}

          {isMatrixRun && (
            <div className="text-xs font-mono text-purple-400 mb-4">
              Comparison group: {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
              {isRuntimeMode && ` x ${selectedLangs.size} language${selectedLangs.size !== 1 ? 's' : ''}`}
            </div>
          )}

          {/* Schedule */}
          <label className="flex items-center gap-3 cursor-pointer mb-4">
            <input
              type="checkbox"
              checked={addSchedule}
              onChange={e => setAddSchedule(e.target.checked)}
              className="w-4 h-4 border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
            />
            <span className="text-sm text-gray-200">Add schedule</span>
          </label>

          {addSchedule && (
            <div className="border border-gray-800 p-4 mb-4">
              <label htmlFor="cron" className="text-xs text-gray-500 mb-1 block">Cron Expression (6-field)</label>
              <input
                id="cron"
                type="text"
                value={cronExpr}
                onChange={e => setCronExpr(e.target.value)}
                className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full font-mono focus:outline-none focus:border-cyan-500"
              />
              <p className="text-xs text-gray-600 mt-1">sec min hour day month weekday -- e.g. 0 0 * * * * = hourly</p>
            </div>
          )}

          {/* Launch buttons */}
          <div className="flex gap-2">
            {!isMatrixRun && (
              <button
                onClick={() => handleSubmit(false)}
                disabled={submitting || configName.trim().length === 0}
                className="border border-gray-700 hover:border-gray-600 text-gray-300 px-4 py-2 text-sm transition-colors disabled:opacity-40"
              >
                Save Config
              </button>
            )}
            <button
              onClick={() => handleSubmit(true)}
              disabled={submitting || configName.trim().length === 0}
              className={`text-white px-6 py-2.5 text-sm font-medium transition-colors ${
                submitting
                  ? 'bg-cyan-700 cursor-wait'
                  : 'bg-cyan-600 hover:bg-cyan-500'
              }`}
            >
              {submitting ? (
                <span className="flex items-center gap-2">
                  <span className="inline-block w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  Launching...
                </span>
              ) : isMatrixRun ? (
                `Launch ${buildComparisonCells().length} Runs`
              ) : (
                'Launch Now'
              )}
            </button>
          </div>
        </div>
      )}

      {/* ── Navigation (ported from production wizard) ── */}
      <div className="flex items-center justify-between mt-10 pt-4 border-t border-gray-800/50">
        <button
          onClick={goBack}
          disabled={step === 0}
          className="text-xs text-gray-500 disabled:text-gray-700 disabled:cursor-not-allowed hover:text-gray-300 transition-colors"
        >
          Back
        </button>

        {step < 5 && !(step === 0 && endpointKind === 'runtime') && (
          <button
            onClick={goNext}
            disabled={!canNext}
            className="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 disabled:bg-gray-800 disabled:text-gray-600 text-white text-xs font-medium transition-colors"
          >
            Next
          </button>
        )}
      </div>
    </div>
  );
}
