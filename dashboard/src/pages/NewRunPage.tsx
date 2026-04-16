import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { useNavigate, Link } from 'react-router-dom';
import { api } from '../api/client';
import { testersApi, type TesterRow } from '../api/testers';
import type { EndpointRef, EndpointKind, Workload, Methodology, TestConfigCreate, ModeGroup, Deployment, ComparisonCell, ComparisonGroupCreate } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ModeSelector } from '../components/common/ModeSelector';
import { PayloadSelector } from '../components/common/PayloadSelector';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { THROUGHPUT_IDS } from '../lib/chart';

type Step = 1 | 2 | 3 | 4;
const ENDPOINT_KINDS: EndpointKind[] = ['network', 'proxy', 'runtime'];

const DEFAULT_METHODOLOGY: Methodology = {
  warmup_runs: 5,
  measured_runs: 30,
  cooldown_ms: 500,
  target_error_pct: 2.0,
  outlier_policy: { policy: 'iqr', k: 1.5 },
  quality_gates: { max_cv_pct: 5.0, min_samples: 10, max_noise_level: 0.1 },
  publication_gates: { max_failure_pct: 5.0, require_all_phases: true },
};

// ── Quick Probe presets ───────────────────────────────────────────────

type ProbePreset = 'quick' | 'standard' | 'full';

const PROBE_PRESETS: Record<ProbePreset, string[]> = {
  quick: ['dns', 'tcp', 'tls', 'http2'],
  standard: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp'],
  full: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp', 'curl', 'pageload', 'pageload2', 'pageload3', 'browser1', 'browser2', 'browser3'],
};

const PROBE_PRESET_LABELS: Record<ProbePreset, { time: string; desc: string }> = {
  quick: { time: '~3s', desc: 'dns, tcp, tls, http2' },
  standard: { time: '~15s', desc: '+ http1, http3, tls-resume, native-tls, udp' },
  full: { time: '~60s', desc: '+ pageload, browser' },
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

/** Extract a clean hostname from user input (URL or bare host). */
function extractHost(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return '';
  try {
    // If it looks like a URL, parse it
    if (trimmed.includes('://')) {
      return new URL(trimmed).hostname;
    }
    // Try adding https:// to parse as URL
    const candidate = new URL(`https://${trimmed}`);
    return candidate.hostname;
  } catch {
    // Fall back to raw input
    return trimmed;
  }
}

// ── Component ───────────────────────────────────────────────────────────

export function NewRunPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Run');

  // ── Quick Probe state ────────────────────────────────────────────────
  const probeInputRef = useRef<HTMLInputElement>(null);
  const [probeUrl, setProbeUrl] = useState('');
  const [probePreset, setProbePreset] = useState<ProbePreset>('quick');
  const [probeSubmitting, setProbeSubmitting] = useState(false);

  // Autofocus the probe URL input on mount
  useEffect(() => {
    probeInputRef.current?.focus();
  }, []);

  const handleProbe = async () => {
    const host = extractHost(probeUrl);
    if (!host) {
      addToast('error', 'Enter a URL or hostname to probe');
      return;
    }

    setProbeSubmitting(true);
    try {
      const presetLabel = probePreset.charAt(0).toUpperCase() + probePreset.slice(1);
      const configName = `Probe: ${host} (${presetLabel})`;

      const endpoint: EndpointRef = { kind: 'network', host };
      const workload: Workload = {
        modes: PROBE_PRESETS[probePreset],
        runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        payload_sizes: [],
        capture_mode: 'headers-only',
      };

      const config: TestConfigCreate = {
        name: configName,
        endpoint,
        workload,
      };

      const created = await api.createTestConfig(projectId, config);
      const run = await api.launchTestConfig(created.id);
      addToast('success', `Probe ${run.id.slice(0, 8)} launched`);
      navigate(`/projects/${projectId}/runs/${run.id}`);
    } catch (e) {
      addToast('error', `Probe failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setProbeSubmitting(false);
    }
  };

  // ── Configured Run state ─────────────────────────────────────────────
  const [configuredExpanded, setConfiguredExpanded] = useState(false);

  // Step tracking
  const [step, setStep] = useState<Step>(1);

  // Step 1: Endpoint
  const [endpointKind, setEndpointKind] = useState<EndpointKind>('network');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('');
  const [proxyEndpointId, setProxyEndpointId] = useState('');
  const [runtimeId, setRuntimeId] = useState('');
  const [language, setLanguage] = useState('');

  // Step 1 -- Proxy: deployed endpoints
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [deploymentsLoading, setDeploymentsLoading] = useState(false);

  // Step 1 -- Runtime: template + language selection
  const [runtimeTemplate, setRuntimeTemplate] = useState<string | null>(null);
  const [runtimeLangs, setRuntimeLangs] = useState<Set<string>>(new Set());
  const [customCloud, setCustomCloud] = useState('Azure');
  const [customRegion, setCustomRegion] = useState('eastus');
  const [customOs, setCustomOs] = useState<'linux' | 'windows'>('linux');

  // Step 1 -- Tester picker (optional, for proxy + runtime)
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [testersLoading, setTestersLoading] = useState(false);
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);
  const [showTesterPicker, setShowTesterPicker] = useState(false);

  // Step 1 -- Matrix builder (comparison group)
  const [compareLanguages, setCompareLanguages] = useState<string[]>([]);
  const [compareRunners, setCompareRunners] = useState<string[]>([]);

  // Step 2: Workload
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http2']));
  const [runs, setRuns] = useState(10);
  const [concurrency, setConcurrency] = useState(1);
  const [timeoutMs, setTimeoutMs] = useState(5000);
  const [selectedPayloads, setSelectedPayloads] = useState<Set<string>>(new Set());

  // Step 3: Methodology (optional)
  const [benchmarkMode, setBenchmarkMode] = useState(false);
  const [methodology, setMethodology] = useState<Methodology>(DEFAULT_METHODOLOGY);

  // Step 4: Config name + schedule
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [submitting, setSubmitting] = useState(false);

  // Load mode groups
  useEffect(() => {
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
  }, []);

  // Load deployments when proxy tab is active
  useEffect(() => {
    if (endpointKind !== 'proxy') return;
    setDeploymentsLoading(true);
    api.getDeployments(projectId)
      .then(deps => setDeployments(deps))
      .catch(() => setDeployments([]))
      .finally(() => setDeploymentsLoading(false));
  }, [endpointKind, projectId]);

  // Load testers when proxy/runtime tab is active (needed for matrix builder + tester picker)
  useEffect(() => {
    if (endpointKind !== 'proxy' && endpointKind !== 'runtime') return;
    setTestersLoading(true);
    testersApi.listTesters(projectId)
      .then(rows => setTesters(rows))
      .catch(() => setTesters([]))
      .finally(() => setTestersLoading(false));
  }, [endpointKind, projectId]);

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
      case 1:
        if (endpointKind === 'network') return host.trim().length > 0;
        if (endpointKind === 'proxy') return proxyEndpointId.trim().length > 0;
        return runtimeId.trim().length > 0 && language.trim().length > 0;
      case 2:
        return selectedModes.size > 0;
      case 3:
        return true;
      case 4:
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

      const workload: Workload = {
        modes: [...selectedModes],
        runs,
        concurrency,
        timeout_ms: timeoutMs,
        payload_sizes: payloadSizes,
        capture_mode: 'headers-only',
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
    <div className="p-4 md:p-6 max-w-3xl">
      <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'New Run' }]} />

      <h2 className="text-xl font-bold text-gray-100 mb-6">New Run</h2>

      {/* ── Quick Probe section ─────────────────────────────────────────── */}
      <section className="mb-8">
        <div className="mb-3">
          <h3 className="text-sm font-semibold text-cyan-400 tracking-wider uppercase">Quick Probe</h3>
          <p className="text-xs text-gray-500 mt-1">Test any URL in ~3 seconds -- no target deployment needed.</p>
        </div>

        <div className="flex flex-col sm:flex-row items-start sm:items-end gap-3">
          {/* URL input */}
          <div className="flex-1 w-full sm:w-auto">
            <label htmlFor="probe-url" className="text-xs text-gray-500 mb-1 block">URL</label>
            <input
              ref={probeInputRef}
              id="probe-url"
              type="text"
              value={probeUrl}
              onChange={e => setProbeUrl(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' && probeUrl.trim()) handleProbe(); }}
              placeholder="e.g. www.cloudflare.com"
              className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm font-mono text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </div>

          {/* Preset selector */}
          <div>
            <label className="text-xs text-gray-500 mb-1 block">Preset</label>
            <div className="flex bg-gray-900 rounded p-0.5">
              {(['quick', 'standard', 'full'] as ProbePreset[]).map(p => (
                <button
                  key={p}
                  onClick={() => setProbePreset(p)}
                  className={`px-3 py-1.5 rounded text-xs font-mono transition-colors ${
                    probePreset === p
                      ? 'bg-cyan-600 text-white'
                      : 'text-gray-400 hover:text-gray-200'
                  }`}
                >
                  {p.charAt(0).toUpperCase() + p.slice(1)}
                </button>
              ))}
            </div>
          </div>

          {/* Run button */}
          <button
            onClick={handleProbe}
            disabled={probeSubmitting || !probeUrl.trim()}
            className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-5 py-2 rounded text-sm font-medium transition-colors whitespace-nowrap"
          >
            {probeSubmitting ? 'Launching...' : 'Run Probe'}
          </button>
        </div>

        {/* Preset descriptions */}
        <div className="mt-3 flex flex-wrap gap-x-6 gap-y-1 text-[11px] font-mono text-gray-600">
          {(['quick', 'standard', 'full'] as ProbePreset[]).map(p => (
            <span key={p} className={probePreset === p ? 'text-gray-400' : ''}>
              <span className="text-gray-500">{p}</span>{' '}
              <span className="text-gray-600">({PROBE_PRESET_LABELS[p].time})</span>{' '}
              {PROBE_PRESET_LABELS[p].desc}
            </span>
          ))}
        </div>
      </section>

      {/* ── Divider ────────────────────────────────────────────────────── */}
      <div className="flex items-center gap-4 mb-8">
        <div className="flex-1 border-t border-gray-800" />
        <span className="text-xs text-gray-600 font-mono">or</span>
        <div className="flex-1 border-t border-gray-800" />
      </div>

      {/* ── Configured Run section ──────────────────────────────────────── */}
      <section>
        <button
          type="button"
          onClick={() => setConfiguredExpanded(!configuredExpanded)}
          className="w-full text-left mb-4 group"
        >
          <div className="flex items-center gap-2">
            <span className={`text-xs text-gray-600 transition-transform ${configuredExpanded ? 'rotate-90' : ''}`}>
              &#9656;
            </span>
            <h3 className="text-sm font-semibold text-gray-300 tracking-wider uppercase group-hover:text-gray-100 transition-colors">
              Configured Run
            </h3>
          </div>
          <p className="text-xs text-gray-600 mt-1 ml-5">
            Full workload test against your deployed targets with optional benchmark methodology.
          </p>
        </button>

        {configuredExpanded && (
          <div className="ml-0">
            {/* Step indicator */}
            <div className="flex items-center gap-1 mb-8 text-xs">
              {[1, 2, 3, 4].map(s => (
                <button
                  key={s}
                  onClick={() => s < step && setStep(s as Step)}
                  disabled={s > step}
                  className={`px-3 py-1.5 rounded transition-colors ${
                    s === step
                      ? 'bg-cyan-600 text-white'
                      : s < step
                        ? 'bg-gray-800 text-cyan-400 hover:bg-gray-700 cursor-pointer'
                        : 'bg-gray-900 text-gray-600 cursor-not-allowed'
                  }`}
                >
                  {s === 1 ? 'Target' : s === 2 ? 'Workload' : s === 3 ? 'Methodology' : 'Save & Launch'}
                </button>
              ))}
            </div>

            {/* Step 1: Target kind */}
            {step === 1 && (
              <div className="space-y-4">
                <div>
                  <label className="text-xs text-gray-500 mb-2 block">Target Type</label>
                  <div className="flex gap-1 bg-gray-900 rounded p-0.5 w-fit">
                    {ENDPOINT_KINDS.map(k => (
                      <button
                        key={k}
                        onClick={() => setEndpointKind(k)}
                        className={`px-4 py-1.5 rounded text-sm transition-colors ${
                          endpointKind === k
                            ? 'bg-cyan-600 text-white'
                            : 'text-gray-400 hover:text-gray-200'
                        }`}
                      >
                        {k.charAt(0).toUpperCase() + k.slice(1)}
                      </button>
                    ))}
                  </div>
                </div>

                {/* -- Network endpoint (unchanged) -- */}
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

                {/* -- Proxy endpoint -- deployed endpoint picker -- */}
                {endpointKind === 'proxy' && (
                  <div>
                    <label className="text-xs text-gray-500 mb-2 block">Select a deployed target</label>

                    {deploymentsLoading && (
                      <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading targets...</p>
                    )}

                    {!deploymentsLoading && deployments.length === 0 && (
                      <div className="border border-dashed border-gray-800 rounded p-4">
                        <p className="text-sm text-gray-300 mb-1">No targets deployed yet.</p>
                        <p className="text-xs text-gray-500 mb-3">
                          Deploy a target from Infrastructure to use it as a proxy target.
                        </p>
                        <Link
                          to={`/projects/${projectId}/vms`}
                          className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors"
                        >
                          Go to Infrastructure / Targets
                        </Link>
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
                                      <span className="text-[10px] font-mono text-gray-500">
                                        {firstEndpoint.provider}
                                      </span>
                                    )}
                                    {firstEndpoint?.region && (
                                      <span className="text-[10px] font-mono text-gray-500">
                                        {firstEndpoint.region}
                                      </span>
                                    )}
                                    {ips.length > 0 && (
                                      <span className="text-[10px] font-mono text-gray-600">
                                        {ips[0]}
                                      </span>
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

                    {!deploymentsLoading && deployments.length > 0 && (
                      <div className="mt-3">
                        <Link
                          to={`/projects/${projectId}/vms`}
                          className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors"
                        >
                          + Deploy new target
                        </Link>
                      </div>
                    )}

                    {/* ── Matrix builder: Compare from multiple runners (proxy) ─── */}
                    {proxyEndpointId && availableRunners.length > 0 && (
                      <div className="border border-gray-800 rounded p-4 space-y-3 mt-4">
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
                          {/* Cloud */}
                          <div className="flex">
                            {CLOUDS.map(c => (
                              <button
                                key={c}
                                onClick={() => {
                                  setCustomCloud(c);
                                  setCustomRegion(REGIONS[c]?.[0] ?? '');
                                }}
                                className={`px-2.5 py-1 text-xs font-mono border transition-colors ${
                                  customCloud === c
                                    ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300 z-10'
                                    : 'border-gray-700 text-gray-500 hover:text-gray-300'
                                } ${c === 'Azure' ? '' : '-ml-px'}`}
                              >
                                {c}
                              </button>
                            ))}
                          </div>

                          {/* OS */}
                          <div className="flex">
                            {(['linux', 'windows'] as const).map(os => (
                              <button
                                key={os}
                                onClick={() => setCustomOs(os)}
                                className={`px-2.5 py-1 text-xs font-mono border transition-colors ${
                                  customOs === os
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

                          {/* Region */}
                          <select
                            value={customRegion}
                            onChange={e => setCustomRegion(e.target.value)}
                            className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
                          >
                            {(REGIONS[customCloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                          </select>
                        </div>

                        {/* Runtime ID (manual for custom) */}
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

                    {/* Language picker (shown once a template is selected) */}
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
                                        selected
                                          ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-300'
                                          : 'border-gray-800 text-gray-500 hover:text-gray-300 hover:border-gray-600'
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

                    {/* ── Matrix builder: Compare across (runtime, non-custom) ─── */}
                    {runtimeTemplate && runtimeTemplate !== 'custom' && (
                      <div className="border border-gray-800 rounded p-4 space-y-4">
                        <h4 className="text-xs font-semibold text-gray-300 uppercase tracking-wider">Compare across</h4>
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                          {/* Languages column */}
                          <div>
                            <label className="text-[10px] font-mono text-gray-500 mb-1.5 block">Languages</label>
                            <div className="space-y-1">
                              {LANGUAGE_GROUPS.flatMap(g => g.entries).map(entry => {
                                const checked = compareLanguages.includes(entry.id);
                                return (
                                  <label key={entry.id} className="flex items-center gap-2 cursor-pointer group">
                                    <input
                                      type="checkbox"
                                      checked={checked}
                                      onChange={() => toggleCompareLanguage(entry.id)}
                                      className="w-3.5 h-3.5 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
                                    />
                                    <span className={`text-xs font-mono ${checked ? 'text-cyan-300' : 'text-gray-500 group-hover:text-gray-300'}`}>
                                      {entry.label}
                                    </span>
                                  </label>
                                );
                              })}
                            </div>
                          </div>

                          {/* Runners column */}
                          <div>
                            <label className="text-[10px] font-mono text-gray-500 mb-1.5 block">Runners</label>
                            {testersLoading && (
                              <p className="text-xs text-gray-600 motion-safe:animate-pulse">Loading runners...</p>
                            )}
                            {!testersLoading && availableRunners.length === 0 && (
                              <p className="text-xs text-gray-600">No runners available. System will auto-assign.</p>
                            )}
                            {!testersLoading && availableRunners.length > 0 && (
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
                            )}
                          </div>
                        </div>

                        {/* Matrix summary */}
                        {matrixCellCount > 1 && (
                          <div className="text-xs font-mono text-purple-400 pt-1">
                            This creates {matrixCellCount} runs ({compareLanguages.length || 1} language{(compareLanguages.length || 1) !== 1 ? 's' : ''} x {compareRunners.length || 1} runner{(compareRunners.length || 1) !== 1 ? 's' : ''})
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )}

                {/* -- Tester picker (optional, for proxy/runtime) -- */}
                {(endpointKind === 'proxy' || endpointKind === 'runtime') && (
                  <div className="border-t border-gray-800 pt-4">
                    <button
                      type="button"
                      onClick={() => setShowTesterPicker(!showTesterPicker)}
                      className="text-xs text-gray-400 hover:text-gray-200 transition-colors"
                    >
                      {showTesterPicker ? '- Hide runner selection' : '+ Select runner (optional, defaults to auto-pick)'}
                    </button>

                    {showTesterPicker && (
                      <div className="mt-3">
                        {testersLoading && (
                          <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading runners...</p>
                        )}

                        {!testersLoading && testers.length === 0 && (
                          <p className="text-xs text-gray-500">No runners available. The system will auto-assign one.</p>
                        )}

                        {!testersLoading && testers.length > 0 && (
                          <div className="space-y-1.5" role="radiogroup" aria-label="Available runners">
                            {/* Auto-pick option */}
                            <label
                              className={`block border p-2.5 cursor-pointer transition-colors ${
                                selectedTesterId === null
                                  ? 'border-cyan-500/50 bg-cyan-500/5'
                                  : 'border-gray-800 hover:border-gray-600'
                              }`}
                            >
                              <div className="flex items-center gap-3">
                                <input
                                  type="radio"
                                  name="tester"
                                  checked={selectedTesterId === null}
                                  onChange={() => setSelectedTesterId(null)}
                                  className="accent-cyan-400"
                                />
                                <span className="text-sm text-gray-300">Auto-pick</span>
                                <span className="text-[10px] text-gray-600">System selects the best available runner</span>
                              </div>
                            </label>

                            {testers.map(row => {
                              const checked = selectedTesterId === row.tester_id;
                              return (
                                <label
                                  key={row.tester_id}
                                  className={`block border p-2.5 cursor-pointer transition-colors ${
                                    checked
                                      ? 'border-cyan-500/50 bg-cyan-500/5'
                                      : 'border-gray-800 hover:border-gray-600'
                                  }`}
                                >
                                  <div className="flex items-center gap-3">
                                    <input
                                      type="radio"
                                      name="tester"
                                      value={row.tester_id}
                                      checked={checked}
                                      onChange={() => setSelectedTesterId(row.tester_id)}
                                      className="accent-cyan-400"
                                    />
                                    <span className="text-sm font-medium text-gray-100 flex-1">{row.name}</span>
                                    <span className="text-[10px] font-mono text-gray-500">
                                      {row.cloud} / {row.region}
                                    </span>
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

                <div className="pt-4">
                  <button
                    onClick={() => setStep(2)}
                    disabled={!canAdvance(1)}
                    className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Workload
                  </button>
                </div>
              </div>
            )}

            {/* Step 2: Workload */}
            {step === 2 && (
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

                <div className="flex gap-2 pt-4">
                  <button onClick={() => setStep(1)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  <button
                    onClick={() => setStep(3)}
                    disabled={!canAdvance(2)}
                    className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Methodology
                  </button>
                </div>
              </div>
            )}

            {/* Step 3: Methodology (optional) */}
            {step === 3 && (
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
                  <button onClick={() => setStep(2)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  <button
                    onClick={() => setStep(4)}
                    className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-2 rounded text-sm transition-colors"
                  >
                    Next: Save & Launch
                  </button>
                </div>
              </div>
            )}

            {/* Step 4: Save & Launch */}
            {step === 4 && (
              <div className="space-y-4">
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

                <label className="flex items-center gap-3 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={addSchedule}
                    onChange={e => setAddSchedule(e.target.checked)}
                    className="w-4 h-4 rounded border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
                  />
                  <span className="text-sm text-gray-200">Also create a recurring schedule</span>
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

                {/* Summary */}
                <div className="border border-gray-800 rounded p-4 text-xs text-gray-400 space-y-1">
                  <p><span className="text-gray-500">Target:</span> {endpointKind}
                    {endpointKind === 'network' && ` / ${host}`}
                    {endpointKind === 'proxy' && selectedDeployment && ` / ${selectedDeployment.name}`}
                    {endpointKind === 'runtime' && runtimeTemplate && ` / ${runtimeTemplate}`}
                  </p>
                  <p><span className="text-gray-500">Modes:</span> {[...selectedModes].join(', ')}</p>
                  <p><span className="text-gray-500">Iterations:</span> {runs} x {concurrency} concurrency</p>
                  {endpointKind === 'runtime' && runtimeLangs.size > 0 && (
                    <p><span className="text-gray-500">Languages:</span> {[...runtimeLangs].join(', ')}</p>
                  )}
                  {selectedTesterId && (
                    <p><span className="text-gray-500">Runner:</span> {testers.find(t => t.tester_id === selectedTesterId)?.name ?? selectedTesterId.slice(0, 8)}</p>
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

                <div className="flex gap-2 pt-4">
                  <button onClick={() => setStep(3)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
                    Back
                  </button>
                  {!isMatrixRun && (
                    <button
                      onClick={() => handleSubmit(false)}
                      disabled={submitting || !canAdvance(4)}
                      className="border border-gray-700 hover:border-gray-600 text-gray-300 px-4 py-2 rounded text-sm transition-colors disabled:opacity-40"
                    >
                      Save Config
                    </button>
                  )}
                  <button
                    onClick={() => handleSubmit(true)}
                    disabled={submitting || !canAdvance(4)}
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
        )}
      </section>
    </div>
  );
}
