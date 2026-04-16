import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate, Link } from 'react-router-dom';
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

type Step = 1 | 2 | 3 | 4 | 5;
const STEP_LABELS: Record<Step, string> = { 1: 'Runner', 2: 'Target', 3: 'Workload', 4: 'Methodology', 5: 'Review' };
type RunnerChoice = 'auto' | 'specific' | 'create';

const DEFAULT_METHODOLOGY: Methodology = {
  warmup_runs: 5,
  measured_runs: 30,
  cooldown_ms: 500,
  target_error_pct: 2.0,
  outlier_policy: { policy: 'iqr', k: 1.5 },
  quality_gates: { max_cv_pct: 5.0, min_samples: 10, max_noise_level: 0.1 },
  publication_gates: { max_failure_pct: 5.0, require_all_phases: true },
};

// ── Runtime template definitions (ported from AppBenchmarkWizardPage) ────

interface RuntimeTemplate {
  id: string;
  name: string;
  description: string;
  defaultLanguages: string[];
  defaultModes: string[];
  methodology: 'quick' | 'standard' | 'rigorous';
}

const RUNTIME_TEMPLATES: RuntimeTemplate[] = [
  {
    id: 'linux-api-stack',
    name: 'Linux API Stack',
    description: 'nginx + Caddy proxies, top 6 languages.',
    defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'java', 'nodejs'],
    defaultModes: ['http1', 'http2', 'http3', 'download', 'upload'],
    methodology: 'standard',
  },
  {
    id: 'windows-api-stack',
    name: 'Windows API Stack',
    description: 'IIS + nginx proxies, .NET ecosystem.',
    defaultLanguages: ['nginx', 'csharp-net48', 'csharp-net8', 'csharp-net8-aot', 'csharp-net9', 'csharp-net9-aot'],
    defaultModes: ['http1', 'http2', 'http3', 'download', 'upload'],
    methodology: 'standard',
  },
  {
    id: 'validation-run',
    name: 'Validation Run',
    description: 'Golden run: Rust + Python, h2 + h3. Validates measurement correctness.',
    defaultLanguages: ['rust', 'python'],
    defaultModes: ['http2', 'http3'],
    methodology: 'standard',
  },
  {
    id: 'low-noise',
    name: 'Low Noise',
    description: 'Single language, extended warmup. For regression tracking.',
    defaultLanguages: ['rust'],
    defaultModes: ['http2'],
    methodology: 'rigorous',
  },
  {
    id: 'custom',
    name: 'Custom',
    description: 'Start from scratch -- pick cloud, region, OS, and languages manually.',
    defaultLanguages: [],
    defaultModes: ['http2'],
    methodology: 'standard',
  },
];

// ── Language catalog (ported from AppBenchmarkWizardPage) ────────────────

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

// ── Cloud/region constants for Custom testbed config ────────────────────

const CLOUDS = ['Azure', 'AWS', 'GCP'] as const;

const REGIONS: Record<string, string[]> = {
  Azure: ['eastus', 'eastus2', 'westus2', 'westus3', 'centralus', 'northeurope', 'westeurope', 'southeastasia', 'japaneast', 'australiaeast'],
  AWS: ['us-east-1', 'us-east-2', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1', 'ap-northeast-1', 'ap-southeast-2'],
  GCP: ['us-central1', 'us-east1', 'us-west1', 'europe-west1', 'europe-west4', 'asia-southeast1', 'asia-northeast1', 'australia-southeast1'],
};

// ── Runner/target creation constants (mirrored from CreateTesterModal) ──

const VM_SIZE_PRESETS: Record<string, { value: string; label: string }[]> = {
  azure: [
    { value: 'Standard_B1s', label: 'Standard_B1s (1 vCPU, 1 GB)' },
    { value: 'Standard_B2s', label: 'Standard_B2s (2 vCPU, 4 GB)' },
    { value: 'Standard_D2s_v3', label: 'Standard_D2s_v3 (2 vCPU, 8 GB)' },
    { value: 'Standard_D4s_v3', label: 'Standard_D4s_v3 (4 vCPU, 16 GB)' },
  ],
  aws: [
    { value: 't3.micro', label: 't3.micro (2 vCPU, 1 GB)' },
    { value: 't3.small', label: 't3.small (2 vCPU, 2 GB)' },
    { value: 't3.medium', label: 't3.medium (2 vCPU, 4 GB)' },
    { value: 't3.large', label: 't3.large (2 vCPU, 8 GB)' },
  ],
  gcp: [
    { value: 'e2-micro', label: 'e2-micro (2 vCPU, 1 GB)' },
    { value: 'e2-small', label: 'e2-small (2 vCPU, 2 GB)' },
    { value: 'e2-medium', label: 'e2-medium (2 vCPU, 4 GB)' },
  ],
};

const DEFAULT_VM_SIZE: Record<string, string> = {
  azure: 'Standard_B2s', aws: 't3.small', gcp: 'e2-small',
};

const OS_OPTIONS: Record<string, { value: string; label: string; variants: string[] }[]> = {
  azure: [
    { value: 'ubuntu-24.04', label: 'Ubuntu 24.04 LTS', variants: ['server', 'desktop'] },
    { value: 'ubuntu-22.04', label: 'Ubuntu 22.04 LTS', variants: ['server'] },
    { value: 'debian-12', label: 'Debian 12', variants: ['server'] },
    { value: 'windows-2022', label: 'Windows Server 2022', variants: ['server'] },
    { value: 'windows-11', label: 'Windows 11', variants: ['desktop'] },
  ],
  aws: [
    { value: 'ubuntu-24.04', label: 'Ubuntu 24.04 LTS', variants: ['server'] },
    { value: 'ubuntu-22.04', label: 'Ubuntu 22.04 LTS', variants: ['server'] },
    { value: 'debian-12', label: 'Debian 12', variants: ['server'] },
    { value: 'windows-2022', label: 'Windows Server 2022', variants: ['server'] },
  ],
  gcp: [
    { value: 'ubuntu-24.04', label: 'Ubuntu 24.04 LTS', variants: ['server'] },
    { value: 'ubuntu-22.04', label: 'Ubuntu 22.04 LTS', variants: ['server'] },
    { value: 'debian-12', label: 'Debian 12', variants: ['server'] },
    { value: 'windows-2022', label: 'Windows Server 2022', variants: ['server'] },
  ],
};

const REGIONS_BY_CLOUD: Record<string, string[]> = {
  azure: ['eastus', 'eastus2', 'westus2', 'westus3', 'centralus', 'northeurope', 'westeurope', 'uksouth', 'francecentral', 'japaneast', 'southeastasia', 'australiaeast'],
  aws: ['us-east-1', 'us-east-2', 'us-west-1', 'us-west-2', 'eu-west-1', 'eu-west-2', 'eu-central-1', 'ap-northeast-1', 'ap-southeast-1', 'ap-southeast-2'],
  gcp: ['us-central1', 'us-east1', 'us-east4', 'us-west1', 'europe-west1', 'europe-west2', 'asia-east1', 'asia-northeast1', 'asia-southeast1'],
};

const DEPLOY_REGIONS: Record<string, string[]> = {
  azure: ['eastus', 'westus2', 'westeurope', 'northeurope', 'southeastasia'],
  aws: ['us-east-1', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1'],
  gcp: ['us-central1', 'us-east1', 'europe-west1', 'europe-west4', 'asia-southeast1'],
};

const HTTP_STACKS = ['nginx', 'iis'] as const;

// ── Methodology presets ─────────────────────────────────────────────────

const METHODOLOGY_FROM_PRESET: Record<string, Partial<Methodology>> = {
  quick: { warmup_runs: 5, measured_runs: 10, target_error_pct: 5.0 },
  standard: { warmup_runs: 10, measured_runs: 50, target_error_pct: 5.0 },
  rigorous: { warmup_runs: 20, measured_runs: 200, target_error_pct: 2.0 },
};

// ── Helpers ─────────────────────────────────────────────────────────────

function deploymentStatusClass(status: string): string {
  switch (status) {
    case 'running': return 'text-green-400 border-green-500/40 bg-green-500/5';
    case 'stopped': case 'stopping': return 'text-gray-400 border-gray-700 bg-gray-800/40';
    case 'error': case 'failed': return 'text-red-400 border-red-500/40 bg-red-500/5';
    case 'creating': case 'starting': return 'text-yellow-300 border-yellow-500/40 bg-yellow-500/5';
    default: return 'text-gray-400 border-gray-700';
  }
}

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
  const [step, setStep] = useState<Step>(1);

  // ── Step 1: Runner ──────────────────────────────────────────────────
  const [runnerChoice, setRunnerChoice] = useState<RunnerChoice>('auto');
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [testersLoading, setTestersLoading] = useState(false);
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Runner creation inline fields
  const [newRunnerCloud, setNewRunnerCloud] = useState('azure');
  const [newRunnerAccountId, setNewRunnerAccountId] = useState('');
  const [newRunnerRegion, setNewRunnerRegion] = useState('');
  const [newRunnerVmSize, setNewRunnerVmSize] = useState('');
  const [newRunnerOs, setNewRunnerOs] = useState('ubuntu-24.04');
  const [newRunnerVariant, setNewRunnerVariant] = useState('server');
  const [newRunnerName, setNewRunnerName] = useState('');

  // Cloud accounts (shared by runner + target creation)
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  const [cloudAccountsLoading, setCloudAccountsLoading] = useState(false);

  // ── Step 2: Target ──────────────────────────────────────────────────
  const [endpointKind, setEndpointKind] = useState<EndpointKind>('network');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('');
  const [proxyEndpointId, setProxyEndpointId] = useState('');
  const [runtimeId, setRuntimeId] = useState('');
  const [language, setLanguage] = useState('');

  // Proxy: deployed endpoints
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [deploymentsLoading, setDeploymentsLoading] = useState(false);
  const [proxySubType, setProxySubType] = useState<'existing' | 'create'>('existing');
  // Proxy create-new fields
  const [newTargetAccountId, setNewTargetAccountId] = useState('');
  const [newTargetRegion, setNewTargetRegion] = useState('');
  const [newTargetOs, setNewTargetOs] = useState<'linux' | 'windows'>('linux');
  const [newTargetHttpStack, setNewTargetHttpStack] = useState('nginx');
  const [newTargetEphemeral, setNewTargetEphemeral] = useState(true);

  // Runtime: template + language selection
  const [runtimeTemplate, setRuntimeTemplate] = useState<string | null>(null);
  const [runtimeLangs, setRuntimeLangs] = useState<Set<string>>(new Set());
  const [customCloud, setCustomCloud] = useState('Azure');
  const [customRegion, setCustomRegion] = useState('eastus');
  const [customOs, setCustomOs] = useState<'linux' | 'windows'>('linux');

  // Matrix builder (comparison group)
  const [compareLanguages, setCompareLanguages] = useState<string[]>([]);
  const [compareRunners, setCompareRunners] = useState<string[]>([]);

  // ── Step 3: Workload ────────────────────────────────────────────────
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http2']));
  const [runs, setRuns] = useState(10);
  const [concurrency, setConcurrency] = useState(1);
  const [timeoutMs, setTimeoutMs] = useState(5000);
  const [selectedPayloads, setSelectedPayloads] = useState<Set<string>>(new Set());
  const [insecure, setInsecure] = useState(false);
  const [connectionReuse, setConnectionReuse] = useState(true);
  const [captureMode, setCaptureMode] = useState<'none' | 'tester' | 'endpoint' | 'both'>('none');

  // Step 4: Methodology (optional)
  const [benchmarkMode, setBenchmarkMode] = useState(false);
  const [methodology, setMethodology] = useState<Methodology>(DEFAULT_METHODOLOGY);

  // Step 5: Config name + schedule
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [submitting, setSubmitting] = useState(false);

  // Load mode groups, agents, deployments, cloud accounts on mount
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
        // Pre-select first account for inline create forms
        if (list.length > 0) {
          setNewRunnerAccountId(list[0].account_id);
          setNewRunnerCloud(list[0].provider);
          setNewTargetAccountId(list[0].account_id);
        }
      })
      .catch(() => setCloudAccounts([]))
      .finally(() => setCloudAccountsLoading(false));
  }, [projectId]);

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

  // ── Runtime template application ─────────────────────────────────────

  const applyRuntimeTemplate = useCallback((tmpl: RuntimeTemplate) => {
    setRuntimeTemplate(tmpl.id);
    setRuntimeLangs(new Set(tmpl.defaultLanguages));
    setSelectedModes(new Set(tmpl.defaultModes));

    // Auto-fill methodology
    if (tmpl.methodology !== 'quick') {
      setBenchmarkMode(true);
      const preset = METHODOLOGY_FROM_PRESET[tmpl.methodology];
      if (preset) {
        setMethodology(m => ({ ...m, ...preset }));
      }
    }

    // For non-custom templates, set a placeholder runtime_id.
    // TODO: When a runtime catalog API exists, look up real runtime IDs
    // based on the template's language set. For now, use template ID as
    // a placeholder that the backend can resolve.
    if (tmpl.id !== 'custom') {
      setRuntimeId(tmpl.id);
      setLanguage(tmpl.defaultLanguages[0] ?? '');
    }
  }, []);

  const toggleRuntimeLang = useCallback((id: string) => {
    setRuntimeLangs(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
    // Update the primary language field to first selected
    setLanguage(prev => {
      if (prev === id) {
        // Removed the current one, pick another
        const remaining = [...runtimeLangs].filter(l => l !== id);
        return remaining[0] ?? '';
      }
      return prev || id;
    });
  }, [runtimeLangs]);

  // ── Matrix builder helpers ───────────────────────────────────────────

  /** Unique runner regions from loaded testers (for matrix runner selection) */
  const availableRunners = useMemo(() => {
    return testers
      .filter(t => t.power_state === 'running' || t.power_state === 'stopped')
      .map(t => ({ id: t.tester_id, label: `${t.name} (${t.cloud} ${t.region})`, region: t.region }));
  }, [testers]);

  /** Runner summary stats */
  const runnerStats = useMemo(() => {
    const online = testers.filter(t => t.power_state === 'running');
    const busy = online.filter(t => t.allocation === 'locked');
    const idle = online.filter(t => t.allocation === 'idle');
    const offline = testers.filter(t => t.power_state !== 'running');
    return { online: online.length, busy: busy.length, idle: idle.length, offline: offline.length };
  }, [testers]);

  /** Selected runner info for summary display */
  const selectedRunner = useMemo(() => {
    if (runnerChoice === 'auto') return null;
    if (runnerChoice === 'create') return null;
    return testers.find(t => t.tester_id === selectedTesterId) ?? null;
  }, [runnerChoice, selectedTesterId, testers]);

  /** Regions for new runner cloud */
  const newRunnerRegions = useMemo(() => {
    return REGIONS_BY_CLOUD[newRunnerCloud] || [];
  }, [newRunnerCloud]);

  /** VM size presets for new runner cloud */
  const newRunnerVmSizes = useMemo(() => {
    return VM_SIZE_PRESETS[newRunnerCloud] || VM_SIZE_PRESETS.azure;
  }, [newRunnerCloud]);

  /** OS options for new runner cloud */
  const newRunnerOsOptions = useMemo(() => {
    return OS_OPTIONS[newRunnerCloud] || [];
  }, [newRunnerCloud]);

  const toggleCompareLanguage = useCallback((lang: string) => {
    setCompareLanguages(prev =>
      prev.includes(lang) ? prev.filter(l => l !== lang) : [...prev, lang],
    );
  }, []);

  const toggleCompareRunner = useCallback((runnerId: string) => {
    setCompareRunners(prev =>
      prev.includes(runnerId) ? prev.filter(r => r !== runnerId) : [...prev, runnerId],
    );
  }, []);

  /** Number of cells the matrix will produce */
  const matrixCellCount = useMemo(() => {
    if (endpointKind === 'runtime') {
      const langs = compareLanguages.length || 1;
      const runners = compareRunners.length || 1;
      return langs * runners;
    }
    if (endpointKind === 'proxy') {
      return compareRunners.length || 1;
    }
    return 1;
  }, [endpointKind, compareLanguages, compareRunners]);

  const isMatrixRun = matrixCellCount > 1;

  // Sync compareLanguages when template changes
  useEffect(() => {
    if (endpointKind === 'runtime' && runtimeTemplate && runtimeTemplate !== 'custom') {
      const tmpl = RUNTIME_TEMPLATES.find(t => t.id === runtimeTemplate);
      if (tmpl) {
        setCompareLanguages(tmpl.defaultLanguages);
      }
    }
  }, [endpointKind, runtimeTemplate]);

  // ── Proxy deployment selection ────────────────────────────────────────

  const selectDeployment = useCallback((dep: Deployment) => {
    setProxyEndpointId(dep.deployment_id);
  }, []);

  // ── Derived: selected deployment info for summary ─────────────────────

  const selectedDeployment = useMemo(
    () => deployments.find(d => d.deployment_id === proxyEndpointId),
    [deployments, proxyEndpointId],
  );

  const buildEndpoint = (): EndpointRef => {
    switch (endpointKind) {
      case 'network':
        return { kind: 'network', host, ...(port ? { port: Number(port) } : {}) };
      case 'proxy':
        return { kind: 'proxy', proxy_endpoint_id: proxyEndpointId };
      case 'runtime':
        return { kind: 'runtime', runtime_id: runtimeId, language };
    }
  };

  const canAdvance = (s: Step): boolean => {
    switch (s) {
      case 1: // Runner
        if (runnerChoice === 'auto') return true;
        if (runnerChoice === 'specific') return selectedTesterId !== null;
        // create: need account, region, name
        return newRunnerAccountId !== '' && newRunnerRegion !== '' && newRunnerName !== '';
      case 2: // Target
        if (endpointKind === 'network') return host.trim().length > 0;
        if (endpointKind === 'proxy') {
          if (proxySubType === 'existing') return proxyEndpointId.trim().length > 0;
          return newTargetAccountId !== '' && newTargetRegion !== '';
        }
        return runtimeId.trim().length > 0 && language.trim().length > 0;
      case 3: // Workload
        return selectedModes.size > 0;
      case 4: // Methodology
        return true;
      case 5: // Launch
        return configName.trim().length > 0;
    }
  };

  /** Build cells for the comparison group from the cartesian product of languages x runners. */
  const buildComparisonCells = (): ComparisonCell[] => {
    const cells: ComparisonCell[] = [];
    const langs = compareLanguages.length > 0 ? compareLanguages : [language];
    const runners = compareRunners.length > 0 ? compareRunners : [undefined];

    if (endpointKind === 'runtime') {
      for (const lang of langs) {
        for (const runnerId of runners) {
          const runnerInfo = runnerId ? availableRunners.find(r => r.id === runnerId) : undefined;
          const label = runnerInfo
            ? `${lang} @ ${runnerInfo.region}`
            : lang;
          cells.push({
            label,
            endpoint: { kind: 'runtime', runtime_id: runtimeId, language: lang },
            ...(runnerId ? { runner_id: runnerId } : {}),
          });
        }
      }
    } else if (endpointKind === 'proxy') {
      for (const runnerId of runners) {
        const runnerInfo = runnerId ? availableRunners.find(r => r.id === runnerId) : undefined;
        const label = runnerInfo ? `from ${runnerInfo.region}` : 'default';
        cells.push({
          label,
          endpoint: { kind: 'proxy', proxy_endpoint_id: proxyEndpointId },
          ...(runnerId ? { runner_id: runnerId } : {}),
        });
      }
    }
    return cells;
  };

  const handleSubmit = async (launchNow: boolean) => {
    setSubmitting(true);
    try {
      // Convert payload size labels to byte values
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

      // ── Matrix path: create a ComparisonGroup ──────────────────────
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

      // ── Single-run path (unchanged) ────────────────────────────────
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

  return (
    <div className="p-4 md:p-6 max-w-[800px] mx-auto">
      <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'New Run' }]} />

      <h2 className="text-xl font-bold text-gray-100 mb-2">New Run</h2>
      <p className="text-xs text-gray-500 mb-6">
        Just testing a URL?{' '}
        <Link
          to={`/projects/${projectId}/runs/new/probe`}
          className="text-cyan-400 hover:text-cyan-300 transition-colors"
        >
          Quick Probe
        </Link>
      </p>

      {/* ── Step indicator ──────────────────────────────────────────────── */}
      <nav aria-label="Wizard steps" className="mb-8">
        <ol className="flex items-center gap-1 text-xs">
          {([1, 2, 3, 4, 5] as Step[]).map(s => (
            <li key={s}>
              <button
                onClick={() => s < step && setStep(s)}
                disabled={s > step}
                className={`px-3 py-1.5 rounded transition-colors ${
                  s === step
                    ? 'bg-cyan-600 text-white'
                    : s < step
                      ? 'bg-gray-800 text-cyan-400 hover:bg-gray-700 cursor-pointer'
                      : 'bg-gray-900 text-gray-600 cursor-not-allowed'
                }`}
              >
                {s}. {STEP_LABELS[s]}
              </button>
            </li>
          ))}
        </ol>
      </nav>

      {/* Step 1: Runner */}
      {step === 1 && (
              <div className="space-y-4">
                <div>
                  <label className="text-xs text-gray-500 mb-2 block">Select a runner</label>

                  {/* Runner summary */}
                  {!testersLoading && testers.length > 0 && (
                    <p className="text-[10px] font-mono text-gray-600 mb-3">
                      {runnerStats.online} runner{runnerStats.online !== 1 ? 's' : ''} online
                      {runnerStats.busy > 0 && <> &middot; {runnerStats.busy} busy</>}
                      {runnerStats.offline > 0 && <> &middot; {runnerStats.offline} offline</>}
                    </p>
                  )}

                  {/* Runner choice cards */}
                  <div className="grid grid-cols-3 gap-3" role="radiogroup" aria-label="Runner selection">
                    <button
                      type="button"
                      onClick={() => { setRunnerChoice('auto'); setSelectedTesterId(null); }}
                      className={`border rounded p-4 text-left transition-colors ${
                        runnerChoice === 'auto'
                          ? 'border-cyan-500 bg-cyan-500/5'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <div className="text-sm font-medium text-gray-100 mb-1">Auto-pick</div>
                      <div className="text-xs text-gray-500">First available online runner</div>
                      {runnerStats.idle > 0 && (
                        <div className="text-xs text-cyan-400 mt-2">{runnerStats.idle} runner{runnerStats.idle !== 1 ? 's' : ''} idle</div>
                      )}
                    </button>

                    <button
                      type="button"
                      onClick={() => setRunnerChoice('specific')}
                      className={`border rounded p-4 text-left transition-colors ${
                        runnerChoice === 'specific'
                          ? 'border-cyan-500 bg-cyan-500/5'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <div className="text-sm font-medium text-gray-100 mb-1">Pick specific</div>
                      <div className="text-xs text-gray-500">Choose a runner by region</div>
                      {runnerStats.online > 0 && (
                        <div className="text-xs text-cyan-400 mt-2">{runnerStats.online} online</div>
                      )}
                    </button>

                    <button
                      type="button"
                      onClick={() => setRunnerChoice('create')}
                      className={`border rounded p-4 text-left transition-colors ${
                        runnerChoice === 'create'
                          ? 'border-cyan-500 bg-cyan-500/5'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <div className="text-sm font-medium text-gray-100 mb-1">Create new</div>
                      <div className="text-xs text-gray-500">Provision a cloud VM (~3 min)</div>
                    </button>
                  </div>

                    {/* Specific runner list */}
                    {runnerChoice === 'specific' && (
                      <div className="ml-6 space-y-1.5">
                        {testersLoading && (
                          <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading runners...</p>
                        )}
                        {!testersLoading && testers.length === 0 && (
                          <p className="text-xs text-gray-500">No runners available. Create one or use Auto-pick.</p>
                        )}
                        {!testersLoading && testers.length > 0 && (
                          <div className="space-y-1" role="radiogroup" aria-label="Available runners">
                            {testers.map(row => {
                              const isOnline = row.power_state === 'running';
                              const isBusy = isOnline && row.allocation === 'locked';
                              const isIdle = isOnline && row.allocation === 'idle';
                              const isOffline = !isOnline;
                              const checked = selectedTesterId === row.tester_id;
                              return (
                                <label
                                  key={row.tester_id}
                                  className={`block border p-2.5 transition-colors ${
                                    isOffline ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'
                                  } ${
                                    checked
                                      ? 'border-cyan-500/50 bg-cyan-500/5'
                                      : 'border-gray-800 hover:border-gray-600'
                                  }`}
                                >
                                  <div className="flex items-center gap-3">
                                    <input
                                      type="radio"
                                      name="runner-specific"
                                      value={row.tester_id}
                                      checked={checked}
                                      disabled={isOffline}
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
                                  {isBusy && checked && row.locked_by_config_id && (
                                    <p className="text-[10px] text-yellow-400 ml-9 mt-1">
                                      Currently running {row.locked_by_config_id.slice(0, 8)}. Your run will queue behind it.
                                    </p>
                                  )}
                                </label>
                              );
                            })}
                          </div>
                        )}
                      </div>
                    )}

                    {/* Create new runner inline form */}
                    {runnerChoice === 'create' && (
                      <div className="ml-6 border border-gray-800 rounded p-4 space-y-3">
                        {/* Cloud Account */}
                        <div>
                          <label htmlFor="new-runner-cloud" className="block text-xs text-gray-400 mb-1">Cloud Account</label>
                          {cloudAccountsLoading ? (
                            <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading accounts...</p>
                          ) : (
                            <select
                              id="new-runner-cloud"
                              value={newRunnerAccountId}
                              onChange={e => {
                                const acctId = e.target.value;
                                setNewRunnerAccountId(acctId);
                                const acct = cloudAccounts.find(a => a.account_id === acctId);
                                if (acct) {
                                  setNewRunnerCloud(acct.provider);
                                  setNewRunnerVmSize(DEFAULT_VM_SIZE[acct.provider] || '');
                                  setNewRunnerRegion('');
                                }
                              }}
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                            >
                              {cloudAccounts.length === 0 && (
                                <option disabled value="">No cloud accounts -- add in Settings</option>
                              )}
                              {cloudAccounts.map(a => (
                                <option key={a.account_id} value={a.account_id}>
                                  {a.provider.toUpperCase()} -- {a.name}
                                  {a.status === 'active' ? '' : ` (${a.status})`}
                                </option>
                              ))}
                            </select>
                          )}
                        </div>

                        {/* Region + VM Size */}
                        <div className="grid grid-cols-2 gap-3">
                          <div>
                            <label htmlFor="new-runner-region" className="block text-xs text-gray-400 mb-1">Region</label>
                            <select
                              id="new-runner-region"
                              value={newRunnerRegion}
                              onChange={e => setNewRunnerRegion(e.target.value)}
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                            >
                              <option value="">Select region</option>
                              {newRunnerRegions.map(r => <option key={r} value={r}>{r}</option>)}
                            </select>
                          </div>
                          <div>
                            <label htmlFor="new-runner-vmsize" className="block text-xs text-gray-400 mb-1">VM Size</label>
                            <select
                              id="new-runner-vmsize"
                              value={newRunnerVmSize}
                              onChange={e => setNewRunnerVmSize(e.target.value)}
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                            >
                              {newRunnerVmSizes.map(p => <option key={p.value} value={p.value}>{p.label}</option>)}
                            </select>
                          </div>
                        </div>

                        {/* OS + Variant */}
                        <div className="grid grid-cols-2 gap-3">
                          <div>
                            <label htmlFor="new-runner-os" className="block text-xs text-gray-400 mb-1">OS</label>
                            <select
                              id="new-runner-os"
                              value={newRunnerOs}
                              onChange={e => {
                                const os = e.target.value;
                                setNewRunnerOs(os);
                                const osDef = newRunnerOsOptions.find(o => o.value === os);
                                if (osDef && !osDef.variants.includes(newRunnerVariant)) {
                                  setNewRunnerVariant(osDef.variants[0]);
                                }
                              }}
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                            >
                              {newRunnerOsOptions.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
                            </select>
                          </div>
                          <div>
                            <label htmlFor="new-runner-variant" className="block text-xs text-gray-400 mb-1">Variant</label>
                            <select
                              id="new-runner-variant"
                              value={newRunnerVariant}
                              onChange={e => setNewRunnerVariant(e.target.value)}
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                            >
                              {(newRunnerOsOptions.find(o => o.value === newRunnerOs)?.variants ?? ['server']).map(v => (
                                <option key={v} value={v}>{v === 'server' ? 'Server' : v === 'desktop' ? 'Desktop' : v}</option>
                              ))}
                            </select>
                          </div>
                        </div>

                        {/* Name */}
                        <div>
                          <label htmlFor="new-runner-name" className="block text-xs text-gray-400 mb-1">Name</label>
                          <input
                            id="new-runner-name"
                            value={newRunnerName}
                            onChange={e => setNewRunnerName(e.target.value)}
                            placeholder={`${newRunnerCloud}-${newRunnerRegion || 'runner'}`}
                            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                          />
                        </div>
                      </div>
                    )}
                </div>

                <div className="pt-4">
                  <button
                    onClick={() => setStep(2)}
                    disabled={!canAdvance(1)}
                    className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Target
                  </button>
                </div>
              </div>
            )}

            {/* Step 2: Target */}
            {step === 2 && (
              <div className="space-y-4">
                <div>
                  <label className="text-xs text-gray-500 mb-2 block">Target Type</label>
                  <div className="grid grid-cols-3 gap-3">
                    <button
                      type="button"
                      onClick={() => setEndpointKind('network')}
                      className={`border rounded p-4 text-left transition-colors ${
                        endpointKind === 'network'
                          ? 'border-cyan-500 bg-cyan-500/5'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <div className="text-sm font-medium text-gray-100 mb-1">Network (URL)</div>
                      <div className="text-xs text-gray-500">Test any public URL or IP</div>
                    </button>

                    <button
                      type="button"
                      onClick={() => setEndpointKind('proxy')}
                      className={`border rounded p-4 text-left transition-colors ${
                        endpointKind === 'proxy'
                          ? 'border-cyan-500 bg-cyan-500/5'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <div className="text-sm font-medium text-gray-100 mb-1">Proxy (target)</div>
                      <div className="text-xs text-gray-500">Use a deployed endpoint</div>
                    </button>

                    <button
                      type="button"
                      onClick={() => setEndpointKind('runtime')}
                      className={`border rounded p-4 text-left transition-colors ${
                        endpointKind === 'runtime'
                          ? 'border-cyan-500 bg-cyan-500/5'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <div className="text-sm font-medium text-gray-100 mb-1">Runtime (stack)</div>
                      <div className="text-xs text-gray-500">Compare language stacks</div>
                    </button>
                  </div>
                </div>

                {/* -- Network endpoint -- */}
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
                        className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
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
                        className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-32 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                      />
                    </div>
                  </div>
                )}

                {/* -- Proxy endpoint -- */}
                {endpointKind === 'proxy' && (
                  <div className="space-y-3">
                    {/* Sub-type selector */}
                    <div className="flex gap-1 bg-gray-900 rounded p-0.5 w-fit">
                      {(['existing', 'create'] as const).map(sub => (
                        <button
                          key={sub}
                          onClick={() => setProxySubType(sub)}
                          className={`px-3 py-1 rounded text-xs transition-colors ${
                            proxySubType === sub
                              ? 'bg-gray-700 text-gray-100'
                              : 'text-gray-500 hover:text-gray-300'
                          }`}
                        >
                          {sub === 'existing' ? 'Use existing target' : 'Create new target'}
                        </button>
                      ))}
                    </div>

                    {/* Existing target picker */}
                    {proxySubType === 'existing' && (
                      <>
                        {deploymentsLoading && (
                          <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading targets...</p>
                        )}

                        {!deploymentsLoading && deployments.length === 0 && (
                          <div className="border border-dashed border-gray-800 rounded p-4">
                            <p className="text-sm text-gray-300 mb-1">No targets deployed yet.</p>
                            <p className="text-xs text-gray-500 mb-2">
                              Switch to "Create new target" to deploy one inline.
                            </p>
                            <button
                              type="button"
                              onClick={() => setProxySubType('create')}
                              className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors"
                            >
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
                              const sameRegion = selectedRunner && firstEndpoint?.region === selectedRunner.region;
                              return (
                                <label
                                  key={dep.deployment_id}
                                  className={`block border p-3 cursor-pointer transition-colors ${
                                    checked
                                      ? 'border-cyan-500/50 bg-cyan-500/5'
                                      : 'border-gray-800 hover:border-gray-600'
                                  }`}
                                >
                                  <div className="flex items-center gap-3">
                                    <input
                                      type="radio"
                                      name="proxy-endpoint"
                                      value={dep.deployment_id}
                                      checked={checked}
                                      onChange={() => selectDeployment(dep)}
                                      className="accent-cyan-400"
                                    />
                                    <div className="flex-1 min-w-0">
                                      <span className="text-sm font-medium text-gray-100">{dep.name}</span>
                                      <div className="flex items-center gap-2 mt-0.5">
                                        {firstEndpoint?.provider && (
                                          <span className="text-[10px] font-mono text-gray-500">{firstEndpoint.provider}</span>
                                        )}
                                        {firstEndpoint?.region && (
                                          <span className="text-[10px] font-mono text-gray-500">{firstEndpoint.region}</span>
                                        )}
                                        {sameRegion && (
                                          <span className="text-[10px] font-mono text-green-500">same region as runner</span>
                                        )}
                                        {ips.length > 0 && (
                                          <span className="text-[10px] font-mono text-gray-600">{ips[0]}</span>
                                        )}
                                      </div>
                                    </div>
                                    <span className={`text-[10px] font-mono px-1.5 py-0.5 border rounded ${deploymentStatusClass(dep.status)}`}>
                                      {dep.status}
                                    </span>
                                  </div>
                                </label>
                              );
                            })}
                          </div>
                        )}
                      </>
                    )}

                    {/* Create new target inline */}
                    {proxySubType === 'create' && (
                      <div className="border border-gray-800 rounded p-4 space-y-3">
                        <div>
                          <label htmlFor="new-target-cloud" className="block text-xs text-gray-400 mb-1">Cloud Account</label>
                          <select
                            id="new-target-cloud"
                            value={newTargetAccountId}
                            onChange={e => {
                              setNewTargetAccountId(e.target.value);
                              const acct = cloudAccounts.find(a => a.account_id === e.target.value);
                              if (acct) {
                                const regions = DEPLOY_REGIONS[acct.provider] ?? [];
                                // Default to runner's region if available
                                const runnerRegion = selectedRunner?.region;
                                setNewTargetRegion(runnerRegion && regions.includes(runnerRegion) ? runnerRegion : regions[0] ?? '');
                              }
                            }}
                            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                          >
                            {cloudAccounts.length === 0 && <option disabled value="">No cloud accounts</option>}
                            {cloudAccounts.map(a => (
                              <option key={a.account_id} value={a.account_id}>
                                {a.provider.toUpperCase()} -- {a.name}
                              </option>
                            ))}
                          </select>
                        </div>

                        <div className="grid grid-cols-2 gap-3">
                          <div>
                            <label htmlFor="new-target-region" className="block text-xs text-gray-400 mb-1">Region</label>
                            <select
                              id="new-target-region"
                              value={newTargetRegion}
                              onChange={e => setNewTargetRegion(e.target.value)}
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
                              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
                            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                          >
                            {HTTP_STACKS.map(s => <option key={s} value={s}>{s.toUpperCase()}</option>)}
                          </select>
                        </div>

                        <div className="space-y-2 pt-1">
                          <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                            <input
                              type="checkbox"
                              checked={newTargetEphemeral}
                              onChange={e => setNewTargetEphemeral(e.target.checked)}
                              className="accent-cyan-500"
                            />
                            Destroy after run
                          </label>
                          <p className="text-[10px] text-gray-600 ml-5">
                            {newTargetEphemeral ? 'Target will be torn down when the run completes.' : 'Target will persist for reuse in future runs.'}
                          </p>
                        </div>
                      </div>
                    )}

                    {/* Matrix builder: proxy runners */}
                    {proxyEndpointId && proxySubType === 'existing' && availableRunners.length > 0 && (
                      <div className="border border-gray-800 rounded p-4 space-y-3">
                        <h4 className="text-xs font-semibold text-gray-300 uppercase tracking-wider">Compare from multiple runners</h4>
                        <div className="space-y-1">
                          {availableRunners.map(runner => {
                            const checked = compareRunners.includes(runner.id);
                            return (
                              <label key={runner.id} className="flex items-center gap-2 cursor-pointer group">
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  onChange={() => toggleCompareRunner(runner.id)}
                                  className="w-3.5 h-3.5 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
                                />
                                <span className={`text-xs font-mono ${checked ? 'text-cyan-300' : 'text-gray-500 group-hover:text-gray-300'}`}>
                                  {runner.label}
                                </span>
                              </label>
                            );
                          })}
                        </div>
                        {compareRunners.length > 1 && (
                          <div className="text-xs font-mono text-purple-400 pt-1">
                            This creates {compareRunners.length} runs (1 target x {compareRunners.length} runners)
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )}

                {/* -- Runtime endpoint -- template gallery + language picker -- */}
                {endpointKind === 'runtime' && (
                  <div className="space-y-4">
                    {/* Template gallery */}
                    <div>
                      <label className="text-xs text-gray-500 mb-2 block">Choose a template</label>
                      <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
                        {RUNTIME_TEMPLATES.map(tmpl => (
                          <button
                            key={tmpl.id}
                            onClick={() => applyRuntimeTemplate(tmpl)}
                            className={`text-left border px-3 py-2.5 transition-colors ${
                              runtimeTemplate === tmpl.id
                                ? 'border-cyan-500/50 bg-cyan-500/5'
                                : 'border-gray-800 hover:border-gray-600'
                            }`}
                          >
                            <div className="text-sm font-medium text-gray-100">{tmpl.name}</div>
                            <div className="text-[11px] text-gray-500 mt-0.5">{tmpl.description}</div>
                            {tmpl.defaultLanguages.length > 0 && (
                              <div className="text-[10px] font-mono text-gray-600 mt-1.5">
                                {tmpl.defaultLanguages.length} lang / {tmpl.defaultModes.length} modes
                              </div>
                            )}
                          </button>
                        ))}
                      </div>
                    </div>

                    {/* Custom testbed configuration */}
                    {runtimeTemplate === 'custom' && (
                      <div className="border border-gray-800 rounded p-4 space-y-3">
                        <h4 className="text-xs font-semibold text-gray-300">Testbed Configuration</h4>
                        <div className="flex items-center gap-2 flex-wrap">
                          <div className="flex">
                            {CLOUDS.map(c => (
                              <button
                                key={c}
                                onClick={() => { setCustomCloud(c); setCustomRegion(REGIONS[c]?.[0] ?? ''); }}
                                className={`px-2.5 py-1 text-xs font-mono border transition-colors ${
                                  customCloud === c ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300 z-10' : 'border-gray-700 text-gray-500 hover:text-gray-300'
                                } ${c === 'Azure' ? '' : '-ml-px'}`}
                              >
                                {c}
                              </button>
                            ))}
                          </div>
                          <div className="flex">
                            {(['linux', 'windows'] as const).map(os => (
                              <button
                                key={os}
                                onClick={() => setCustomOs(os)}
                                className={`px-2.5 py-1 text-xs font-mono border transition-colors ${
                                  customOs === os
                                    ? os === 'linux' ? 'bg-green-500/10 border-green-500/40 text-green-300 z-10' : 'bg-blue-500/10 border-blue-500/40 text-blue-300 z-10'
                                    : 'border-gray-700 text-gray-500 hover:text-gray-300'
                                } ${os === 'linux' ? '' : '-ml-px'}`}
                              >
                                {os === 'linux' ? 'Linux' : 'Windows'}
                              </button>
                            ))}
                          </div>
                          <select
                            value={customRegion}
                            onChange={e => setCustomRegion(e.target.value)}
                            className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
                          >
                            {(REGIONS[customCloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                          </select>
                        </div>
                        <div>
                          <label htmlFor="runtimeId" className="text-xs text-gray-500 mb-1 block">Runtime ID</label>
                          <input
                            id="runtimeId"
                            type="text"
                            value={runtimeId}
                            onChange={e => setRuntimeId(e.target.value)}
                            placeholder="UUID or template slug"
                            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                          />
                        </div>
                      </div>
                    )}

                    {/* Language picker */}
                    {runtimeTemplate && (
                      <div>
                        <label className="text-xs text-gray-500 mb-2 block">
                          Languages
                          <span className="text-gray-600 ml-2">{runtimeLangs.size} selected</span>
                        </label>
                        <div className="space-y-3">
                          {LANGUAGE_GROUPS.map(group => (
                            <div key={group.label}>
                              <div className="text-[10px] font-mono text-gray-600 mb-1">{group.label}</div>
                              <div className="flex flex-wrap gap-1">
                                {group.entries.map(entry => {
                                  const selected = runtimeLangs.has(entry.id);
                                  return (
                                    <button
                                      key={entry.id}
                                      onClick={() => toggleRuntimeLang(entry.id)}
                                      className={`px-2 py-1 text-xs font-mono border transition-colors ${
                                        selected ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-300' : 'border-gray-800 text-gray-500 hover:text-gray-300 hover:border-gray-600'
                                      }`}
                                    >
                                      {entry.label}
                                    </button>
                                  );
                                })}
                              </div>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}

                    {/* Matrix builder */}
                    {runtimeTemplate && runtimeTemplate !== 'custom' && (
                      <div className="border border-gray-800 rounded p-4 space-y-4">
                        <h4 className="text-xs font-semibold text-gray-300 uppercase tracking-wider">Compare across</h4>
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                          <div>
                            <label className="text-[10px] font-mono text-gray-500 mb-1.5 block">Languages</label>
                            <div className="space-y-1">
                              {LANGUAGE_GROUPS.flatMap(g => g.entries).map(entry => {
                                const checked = compareLanguages.includes(entry.id);
                                return (
                                  <label key={entry.id} className="flex items-center gap-2 cursor-pointer group">
                                    <input type="checkbox" checked={checked} onChange={() => toggleCompareLanguage(entry.id)} className="w-3.5 h-3.5 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50" />
                                    <span className={`text-xs font-mono ${checked ? 'text-cyan-300' : 'text-gray-500 group-hover:text-gray-300'}`}>{entry.label}</span>
                                  </label>
                                );
                              })}
                            </div>
                          </div>
                          <div>
                            <label className="text-[10px] font-mono text-gray-500 mb-1.5 block">Runners</label>
                            {testersLoading && <p className="text-xs text-gray-600 motion-safe:animate-pulse">Loading runners...</p>}
                            {!testersLoading && availableRunners.length === 0 && <p className="text-xs text-gray-600">No runners available.</p>}
                            {!testersLoading && availableRunners.length > 0 && (
                              <div className="space-y-1">
                                {availableRunners.map(runner => {
                                  const checked = compareRunners.includes(runner.id);
                                  return (
                                    <label key={runner.id} className="flex items-center gap-2 cursor-pointer group">
                                      <input type="checkbox" checked={checked} onChange={() => toggleCompareRunner(runner.id)} className="w-3.5 h-3.5 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50" />
                                      <span className={`text-xs font-mono ${checked ? 'text-cyan-300' : 'text-gray-500 group-hover:text-gray-300'}`}>{runner.label}</span>
                                    </label>
                                  );
                                })}
                              </div>
                            )}
                          </div>
                        </div>
                        {matrixCellCount > 1 && (
                          <div className="text-xs font-mono text-purple-400 pt-1">
                            This creates {matrixCellCount} runs ({compareLanguages.length || 1} language{(compareLanguages.length || 1) !== 1 ? 's' : ''} x {compareRunners.length || 1} runner{(compareRunners.length || 1) !== 1 ? 's' : ''})
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )}

                <div className="flex gap-2 pt-4">
                  <button onClick={() => setStep(1)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  <button
                    onClick={() => setStep(3)}
                    disabled={!canAdvance(2)}
                    className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Workload
                  </button>
                </div>
              </div>
            )}

            {/* Step 3: Workload */}
            {step === 3 && (
              <div className="space-y-4">
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
                      className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
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
                      className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
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
                      className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
                    />
                  </div>
                </div>

                {hasThroughput && (
                  <div>
                    <label className="text-xs text-gray-500 mb-2 block">Payload Sizes</label>
                    <PayloadSelector
                      selected={selectedPayloads}
                      onToggle={handlePayloadToggle}
                    />
                  </div>
                )}

                {/* Advanced options (collapsible) */}
                <details className="border border-gray-800 rounded">
                  <summary className="px-4 py-3 text-xs font-semibold text-gray-400 uppercase tracking-wider cursor-pointer hover:text-gray-300 transition-colors select-none">
                    Advanced
                  </summary>
                  <div className="px-4 pb-4 space-y-3">
                    <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                      <input
                        type="checkbox"
                        checked={insecure}
                        onChange={e => setInsecure(e.target.checked)}
                        className="accent-cyan-500"
                      />
                      Allow insecure HTTPS (skip TLS verification)
                    </label>

                    <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                      <input
                        type="checkbox"
                        checked={connectionReuse}
                        onChange={e => setConnectionReuse(e.target.checked)}
                        className="accent-cyan-500"
                      />
                      Reuse connections (keep-alive)
                    </label>

                    <div>
                      <label htmlFor="capture-mode" className="block text-xs text-gray-400 mb-1">Capture mode</label>
                      <select
                        id="capture-mode"
                        value={captureMode}
                        onChange={e => setCaptureMode(e.target.value as typeof captureMode)}
                        className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        <option value="none">None</option>
                        <option value="tester">Tester-side</option>
                        <option value="endpoint">Endpoint-side</option>
                        <option value="both">Both</option>
                      </select>
                    </div>
                  </div>
                </details>

                <div className="flex gap-2 pt-4">
                  <button onClick={() => setStep(2)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  <button
                    onClick={() => setStep(4)}
                    disabled={!canAdvance(3)}
                    className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Methodology
                  </button>
                </div>
              </div>
            )}

            {/* Step 4: Methodology (optional) */}
            {step === 4 && (
              <div className="space-y-4">
                <label className="flex items-center gap-3 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={benchmarkMode}
                    onChange={e => setBenchmarkMode(e.target.checked)}
                    className="w-4 h-4 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
                  />
                  <div>
                    <span className="text-sm text-gray-200">Enable benchmark mode</span>
                    <p className="text-xs text-gray-500">Adds warmup, measured iterations, quality gates, and generates a publishable artifact.</p>
                  </div>
                </label>

                {benchmarkMode && (
                  <div className="border border-gray-800 rounded p-4 space-y-3">
                    <div className="grid grid-cols-2 gap-4">
                      <div>
                        <label htmlFor="warmup" className="text-xs text-gray-500 mb-1 block">Warmup Runs</label>
                        <input
                          id="warmup"
                          type="number"
                          min={0}
                          value={methodology.warmup_runs}
                          onChange={e => setMethodology(m => ({ ...m, warmup_runs: Number(e.target.value) }))}
                          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                      <div>
                        <label htmlFor="measured" className="text-xs text-gray-500 mb-1 block">Measured Runs</label>
                        <input
                          id="measured"
                          type="number"
                          min={1}
                          value={methodology.measured_runs}
                          onChange={e => setMethodology(m => ({ ...m, measured_runs: Number(e.target.value) }))}
                          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                      <div>
                        <label htmlFor="cooldown" className="text-xs text-gray-500 mb-1 block">Cooldown (ms)</label>
                        <input
                          id="cooldown"
                          type="number"
                          min={0}
                          value={methodology.cooldown_ms}
                          onChange={e => setMethodology(m => ({ ...m, cooldown_ms: Number(e.target.value) }))}
                          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                      <div>
                        <label htmlFor="targetErr" className="text-xs text-gray-500 mb-1 block">Target Error %</label>
                        <input
                          id="targetErr"
                          type="number"
                          step="0.1"
                          min={0}
                          value={methodology.target_error_pct}
                          onChange={e => setMethodology(m => ({ ...m, target_error_pct: Number(e.target.value) }))}
                          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                    </div>
                    <div className="text-xs text-gray-600 pt-2">
                      Quality gates: CV &lt; {methodology.quality_gates.max_cv_pct}%, min {methodology.quality_gates.min_samples} samples.
                      Publication gate: failure rate &lt; {methodology.publication_gates.max_failure_pct}%.
                    </div>
                  </div>
                )}

                <div className="flex gap-2 pt-4">
                  <button onClick={() => setStep(3)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  <button
                    onClick={() => setStep(5)}
                    className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Launch
                  </button>
                </div>
              </div>
            )}

            {/* Step 5: Review & Launch */}
            {step === 5 && (
              <div className="space-y-4">
                {/* Summary card */}
                <div className="border border-gray-800 rounded p-4 text-xs text-gray-400 space-y-1.5">
                  <p>
                    <span className="text-gray-500">Runner:</span>{' '}
                    {runnerChoice === 'auto' && 'Auto-pick (first available)'}
                    {runnerChoice === 'specific' && (selectedRunner ? `${selectedRunner.name} (${selectedRunner.cloud} / ${selectedRunner.region})` : 'None selected')}
                    {runnerChoice === 'create' && `New: ${newRunnerName || 'unnamed'} (${newRunnerCloud} / ${newRunnerRegion})`}
                  </p>
                  <p>
                    <span className="text-gray-500">Target:</span> {endpointKind}
                    {endpointKind === 'network' && ` / ${host}`}
                    {endpointKind === 'proxy' && proxySubType === 'existing' && selectedDeployment && ` / ${selectedDeployment.name}`}
                    {endpointKind === 'proxy' && proxySubType === 'create' && ` / new (${newTargetOs}, ${newTargetHttpStack})`}
                    {endpointKind === 'runtime' && runtimeTemplate && ` / ${runtimeTemplate}`}
                  </p>
                  <p><span className="text-gray-500">Modes:</span> {[...selectedModes].join(', ')}</p>
                  <p><span className="text-gray-500">Iterations:</span> {runs} x {concurrency} concurrency</p>
                  {insecure && <p className="text-yellow-400">Insecure mode (TLS verification disabled)</p>}
                  {!connectionReuse && <p className="text-gray-500">Connection reuse disabled</p>}
                  {captureMode !== 'none' && <p><span className="text-gray-500">Capture:</span> {captureMode}</p>}
                  {endpointKind === 'runtime' && runtimeLangs.size > 0 && (
                    <p><span className="text-gray-500">Languages:</span> {[...runtimeLangs].join(', ')}</p>
                  )}
                  {benchmarkMode && <p><span className="text-purple-400">Benchmark mode enabled</span> -- {methodology.measured_runs} measured runs</p>}
                  {isMatrixRun && (
                    <p className="text-purple-400 font-medium">
                      Comparison group: {matrixCellCount} runs
                      {endpointKind === 'runtime' && ` (${compareLanguages.length || 1} lang x ${compareRunners.length || 1} runner${(compareRunners.length || 1) !== 1 ? 's' : ''})`}
                      {endpointKind === 'proxy' && ` (1 target x ${compareRunners.length} runners)`}
                    </p>
                  )}
                </div>

                {/* Warnings */}
                {runnerChoice === 'specific' && selectedRunner?.allocation === 'locked' && (
                  <div className="bg-yellow-500/10 border border-yellow-500/30 rounded p-2 text-xs text-yellow-300">
                    Runner {selectedRunner.name} is busy. Your run will be queued.
                  </div>
                )}
                {runnerChoice === 'create' && (
                  <div className="bg-blue-500/10 border border-blue-500/30 rounded p-2 text-xs text-blue-300">
                    Runner will be provisioned first (~3 min).
                  </div>
                )}
                {endpointKind === 'proxy' && proxySubType === 'create' && (
                  <div className="bg-blue-500/10 border border-blue-500/30 rounded p-2 text-xs text-blue-300">
                    Target will be deployed first (~2 min).
                  </div>
                )}

                {/* Schedule */}
                <label className="flex items-center gap-3 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={addSchedule}
                    onChange={e => setAddSchedule(e.target.checked)}
                    className="w-4 h-4 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
                  />
                  <span className="text-sm text-gray-200">Add schedule</span>
                </label>

                {addSchedule && (
                  <div className="border border-gray-800 rounded p-4">
                    <label htmlFor="cron" className="text-xs text-gray-500 mb-1 block">Cron Expression (6-field)</label>
                    <input
                      id="cron"
                      type="text"
                      value={cronExpr}
                      onChange={e => setCronExpr(e.target.value)}
                      className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full font-mono focus:outline-none focus:border-cyan-500"
                    />
                    <p className="text-xs text-gray-600 mt-1">sec min hour day month weekday -- e.g. 0 0 * * * * = hourly</p>
                  </div>
                )}

                {/* Auto-teardown (only for ephemeral targets) */}
                {endpointKind === 'proxy' && proxySubType === 'create' && newTargetEphemeral && (
                  <label className="flex items-center gap-3 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={newTargetEphemeral}
                      onChange={e => setNewTargetEphemeral(e.target.checked)}
                      className="w-4 h-4 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
                    />
                    <span className="text-sm text-gray-200">Auto-teardown target after run</span>
                  </label>
                )}

                {/* Config name */}
                <div>
                  <label htmlFor="configName" className="text-xs text-gray-500 mb-1 block">Configuration Name</label>
                  <input
                    id="configName"
                    type="text"
                    value={configName}
                    onChange={e => setConfigName(e.target.value)}
                    placeholder="e.g. cloudflare-http2-daily"
                    className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                  />
                </div>

                <div className="flex gap-2 pt-4">
                  <button onClick={() => setStep(4)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  {!isMatrixRun && (
                    <button
                      onClick={() => handleSubmit(false)}
                      disabled={submitting || !canAdvance(5)}
                      className="border border-gray-700 hover:border-gray-600 text-gray-300 px-4 py-2 rounded text-sm transition-colors disabled:opacity-40"
                    >
                      Save Config
                    </button>
                  )}
                  <button
                    onClick={() => handleSubmit(true)}
                    disabled={submitting || !canAdvance(5)}
                    className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    {submitting
                      ? 'Launching...'
                      : isMatrixRun
                        ? `Launch ${matrixCellCount} Runs`
                        : 'Launch Now'}
                  </button>
                </div>
              </div>
            )}
    </div>
  );
}
