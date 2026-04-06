import { useState, useMemo, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkTestbedConfig, BenchmarkVmCatalogEntry } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';

// ── Constants ────────────────────────────────────────────────────────────

const STEP_LABELS = ['Template', 'Testbeds', 'Languages', 'Methodology', 'Review'] as const;

interface TemplateOption {
  id: string;
  name: string;
  description: string;
  defaultTestbedCount: number;
  defaultOs: 'linux' | 'windows' | null;
  defaultLanguages: string[];
  methodology: string;
}

const TEMPLATES: TemplateOption[] = [
  {
    id: 'linux-api-stack',
    name: 'Linux API Stack',
    description: 'nginx + Caddy proxies, top 6 languages.',
    defaultTestbedCount: 1,
    defaultOs: 'linux' as const,
    defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'java', 'nodejs'],
    methodology: 'standard',
  },
  {
    id: 'windows-api-stack',
    name: 'Windows API Stack',
    description: 'IIS + nginx proxies, .NET ecosystem.',
    defaultTestbedCount: 1,
    defaultOs: 'windows' as const,
    defaultLanguages: ['nginx', 'csharp-net48', 'csharp-net8', 'csharp-net8-aot', 'csharp-net9', 'csharp-net9-aot', 'csharp-net10', 'csharp-net10-aot'],
    methodology: 'standard',
  },
  {
    id: 'proxy-comparison',
    name: 'Proxy Comparison',
    description: 'All OS-compatible proxies, 3 languages.',
    defaultTestbedCount: 1,
    defaultOs: 'linux' as const,
    defaultLanguages: ['nginx', 'rust', 'python'],
    methodology: 'standard',
  },
  {
    id: 'validation-run',
    name: 'Validation Run',
    description: 'Golden run: nginx proxy, Rust + Python, h2 + h3. Validates measurement correctness.',
    defaultTestbedCount: 1,
    defaultOs: 'linux' as const,
    defaultLanguages: ['rust', 'python'],
    methodology: 'standard',
  },
  {
    id: 'low-noise',
    name: 'Low Noise',
    description: 'Single proxy + language, extended warmup. For regression tracking.',
    defaultTestbedCount: 1,
    defaultOs: 'linux' as const,
    defaultLanguages: ['rust'],
    methodology: 'rigorous',
  },
  {
    id: 'custom',
    name: 'Custom',
    description: 'Start from scratch.',
    defaultTestbedCount: 0,
    defaultOs: null,
    defaultLanguages: ['nginx'],
    methodology: 'standard',
  },
];

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
  nginx: 'nginx',
  iis: 'IIS',
  caddy: 'Caddy',
  traefik: 'Traefik',
  haproxy: 'HAProxy',
  apache: 'Apache',
};

const TESTER_OS_OPTIONS = [
  { id: 'server', label: 'Server (headless)' },
  { id: 'desktop-linux', label: 'Desktop Linux' },
  { id: 'desktop-windows', label: 'Desktop Windows' },
] as const;

const TEMPLATE_DEFAULT_PROXIES: Record<string, string[]> = {
  'linux-api-stack': ['nginx', 'caddy'],
  'windows-api-stack': ['iis', 'nginx'],
  'proxy-comparison': ['nginx', 'caddy', 'traefik', 'haproxy', 'apache'],
  'validation-run': ['nginx'],
  'low-noise': ['nginx'],
  'custom': [],
};

interface LanguageEntry {
  id: string;
  label: string;
  group: string;
}

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
      { id: 'csharp-net6', label: 'C# .NET 6', group: 'Managed' },
      { id: 'csharp-net7', label: 'C# .NET 7', group: 'Managed' },
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

interface MethodologyPreset {
  id: string;
  label: string;
  warmup: number;
  measured: number;
  targetError: number | null;
}

const METHODOLOGY_PRESETS: MethodologyPreset[] = [
  { id: 'quick', label: 'Quick', warmup: 5, measured: 10, targetError: null },
  { id: 'standard', label: 'Standard', warmup: 10, measured: 50, targetError: 5 },
  { id: 'rigorous', label: 'Rigorous', warmup: 10, measured: 200, targetError: 2 },
];

const DEFAULT_MODES = ['http1', 'http2', 'http3', 'download', 'upload'];

// ── Testbed state ───────────────────────────────────────────────────────

interface TestbedState {
  key: number;
  cloud: string;
  region: string;
  topology: string;
  vmSize: string;
  os: 'linux' | 'windows';
  useExisting: boolean;
  existingVmId: string;
  proxies: string[];
  testerOs: string;
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
    useExisting: false,
    existingVmId: '',
    proxies: proxies ?? [],
    testerOs: 'server',
  };
}

// ── Component ────────────────────────────────────────────────────────────

export function AppBenchmarkWizardPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  usePageTitle('Application Benchmark Wizard');

  const [step, setStep] = useState(0);

  // Step 1: Template
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);

  // Step 2: Testbeds
  const [testbedKey, setTestbedKey] = useState(0);
  const [testbeds, setTestbeds] = useState<TestbedState[]>([]);

  // Step 3: Languages
  const [selectedLangs, setSelectedLangs] = useState<Set<string>>(new Set(['nginx']));

  // Step 4: Methodology
  const [methodPreset, setMethodPreset] = useState<string>('standard');
  const [warmup, setWarmup] = useState(10);
  const [measured, setMeasured] = useState(50);
  const [targetError, setTargetError] = useState<number | null>(5);
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(DEFAULT_MODES));
  const [showAdvanced, setShowAdvanced] = useState(false);

  // Step 5: Review
  const [autoTeardown, setAutoTeardown] = useState(true);
  const [benchmarkName, setBenchmarkName] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  // Catalog (loaded lazily for step 2)
  const [catalog, setCatalog] = useState<BenchmarkVmCatalogEntry[]>([]);
  const [catalogLoaded, setCatalogLoaded] = useState(false);

  // Proxy validation warning
  const [proxyWarning, setProxyWarning] = useState(false);

  const loadCatalog = useCallback(() => {
    if (catalogLoaded || !projectId) return;
    api.listBenchmarkCatalog(projectId)
      .then(data => {
        setCatalog(data);
        setCatalogLoaded(true);
        // If testbed has useExisting checked, auto-select the first matching VM
        setTestbeds(prev => prev.map(testbed => {
          if (testbed.useExisting && !testbed.existingVmId) {
            const matches = data.filter(vm =>
              vm.cloud.toLowerCase() === testbed.cloud.toLowerCase() &&
              vm.region.toLowerCase() === testbed.region.toLowerCase()
            );
            if (matches.length >= 1) {
              return { ...testbed, existingVmId: matches[0].vm_id };
            }
          }
          return testbed;
        }));
      })
      .catch(() => { setCatalogLoaded(true); });
  }, [projectId, catalogLoaded]);

  // ── Template selection ─────────────────────────────────────────────────

  const applyTemplate = (tmpl: TemplateOption) => {
    setSelectedTemplate(tmpl.id);

    const defaultProxies = TEMPLATE_DEFAULT_PROXIES[tmpl.id] ?? [];

    // Pre-fill testbeds
    const newTestbeds: TestbedState[] = [];
    if (tmpl.id !== 'custom') {
      const k = testbedKey;
      setTestbedKey(k + 1);
      newTestbeds.push(makeTestbed(k, 'Azure', tmpl.defaultOs ?? 'linux', defaultProxies));
    }
    setTestbeds(newTestbeds);

    // Languages
    setSelectedLangs(new Set(tmpl.defaultLanguages));

    // Methodology
    const preset = METHODOLOGY_PRESETS.find(p => p.id === tmpl.methodology) ?? METHODOLOGY_PRESETS[1];
    setMethodPreset(preset.id);
    setWarmup(preset.warmup);
    setMeasured(preset.measured);
    setTargetError(preset.targetError);

    // Validation Run: only http2 + http3 modes
    if (tmpl.id === 'validation-run') {
      setSelectedModes(new Set(['http2', 'http3']));
    }
    // Low Noise: extended warmup (20 cycles)
    if (tmpl.id === 'low-noise') {
      setWarmup(20);
    }

    setProxyWarning(false);
    loadCatalog();
    setStep(1);
  };

  // ── Testbed helpers ───────────────────────────────────────────────────

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
      // Reset region when cloud changes
      if (patch.cloud && patch.cloud !== c.cloud) {
        updated.region = REGIONS[patch.cloud]?.[0] ?? '';
      }
      // When OS changes, filter out invalid proxies
      if (patch.os && patch.os !== c.os) {
        const validProxies = patch.os === 'windows'
          ? WINDOWS_PROXIES as readonly string[]
          : LINUX_PROXIES as readonly string[];
        updated.proxies = updated.proxies.filter(p => validProxies.includes(p));
      }
      return updated;
    }));
  };

  // ── Language helpers ───────────────────────────────────────────────────

  const toggleLang = (id: string) => {
    setSelectedLangs(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      // nginx always stays
      next.add('nginx');
      return next;
    });
  };

  const setLangShortcut = (ids: string[]) => {
    const s = new Set(ids);
    s.add('nginx');
    setSelectedLangs(s);
  };

  // ── Methodology preset apply ──────────────────────────────────────────

  const applyMethodPreset = (presetId: string) => {
    const p = METHODOLOGY_PRESETS.find(m => m.id === presetId);
    if (!p) return;
    setMethodPreset(presetId);
    setWarmup(p.warmup);
    setMeasured(p.measured);
    setTargetError(p.targetError);
  };

  const toggleMode = (mode: string) => {
    setSelectedModes(prev => {
      const next = new Set(prev);
      if (next.has(mode)) next.delete(mode);
      else next.add(mode);
      return next;
    });
  };

  // ── Navigation ─────────────────────────────────────────────────────────

  const canNext = useMemo(() => {
    if (step === 0) return selectedTemplate !== null;
    if (step === 1) return testbeds.length > 0 && testbeds.every(c => (!c.useExisting || c.existingVmId !== '') && c.proxies.length > 0);
    if (step === 2) return selectedLangs.size > 0;
    if (step === 3) return warmup > 0 && measured > 0 && selectedModes.size > 0;
    return true;
  }, [step, selectedTemplate, testbeds, selectedLangs.size, warmup, measured, selectedModes.size]);

  const goNext = () => {
    if (step === 0 && selectedTemplate === null) return;
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
          return { ...tb, os: 'windows', proxies: tb.proxies.filter(p => validProxies.includes(p)) };
        }
        return tb;
      }));
    }
    if (step < STEP_LABELS.length - 1) {
      const next = step + 1;
      if (next === 1) loadCatalog();
      setStep(next);
    }
  };

  const goBack = () => {
    if (step > 0) setStep(step - 1);
  };

  // ── Submit ─────────────────────────────────────────────────────────────

  const buildPayload = () => {
    const catalogVms = catalog;
    const testbedConfigs: BenchmarkTestbedConfig[] = testbeds.map(tb => ({
      cloud: tb.cloud.toLowerCase(),
      region: tb.region,
      topology: tb.topology,
      vm_size: tb.vmSize,
      os: tb.os,
      existing_vm_ip: tb.useExisting ? (catalogVms.find(v => v.vm_id === tb.existingVmId)?.ip ?? null) : null,
      languages: Array.from(selectedLangs),
      proxies: tb.proxies,
      tester_os: tb.testerOs,
    }));

    return {
      name: benchmarkName.trim() || `Application benchmark ${new Date().toISOString().slice(0, 10)}`,
      template: selectedTemplate,
      benchmark_type: 'application' as const,
      testbeds: testbedConfigs,
      languages: Array.from(selectedLangs),
      methodology: {
        preset: methodPreset,
        warmup_runs: warmup,
        measured_runs: measured,
        target_error_percent: targetError,
        modes: Array.from(selectedModes),
      },
      auto_teardown: autoTeardown,
    };
  };

  const handleLaunch = async () => {
    if (!projectId) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      const payload = buildPayload();
      // Testbeds without existing_vm_ip will be auto-provisioned by the orchestrator
      const { config_id } = await api.createBenchmarkConfig(projectId, payload);
      const result = await api.launchBenchmarkConfig(projectId, config_id);
      if (result.error) {
        setSubmitError(result.message ?? result.error);
        return;
      }
      navigate(`/projects/${projectId}/benchmark-progress/${config_id}`);
    } catch (err) {
      setSubmitError(String(err));
    } finally {
      setSubmitting(false);
    }
  };

  // ── Total estimates ────────────────────────────────────────────────────

  const totalVMs = testbeds.filter(c => !c.useExisting).length;
  const totalExisting = testbeds.filter(c => c.useExisting).length;
  const totalLanguages = selectedLangs.size;
  const totalProxies = new Set(testbeds.flatMap(tb => tb.proxies)).size;
  const totalCombinations = testbeds.length * totalLanguages * (totalProxies || 1);

  // ── Render ─────────────────────────────────────────────────────────────
  return (
    <div className="p-4 md:p-6 max-w-5xl">
      {/* Header */}
      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Application Benchmark Wizard</h2>
      </div>

      {/* Stepper */}
      <div className="flex items-center gap-0.5 mb-8 font-mono text-xs">
        {STEP_LABELS.map((label, i) => (
          <button
            key={label}
            onClick={() => { if (i < step) setStep(i); }}
            disabled={i > step}
            className={`px-2 py-1 transition-colors ${
              i === step
                ? 'text-cyan-300'
                : i < step
                  ? 'text-gray-500 hover:text-gray-300 cursor-pointer'
                  : 'text-gray-700 cursor-not-allowed'
            }`}
          >
            {i + 1}. {label}
          </button>
        ))}
      </div>

      {/* ── Step 0: Template ── */}
      {step === 0 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Choose a template</h3>
          <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
            {TEMPLATES.map(tmpl => (
              <button
                key={tmpl.id}
                onClick={() => applyTemplate(tmpl)}
                className={`text-left border px-3 py-2.5 transition-colors ${
                  selectedTemplate === tmpl.id
                    ? 'border-cyan-500/50 bg-cyan-500/5'
                    : 'border-gray-800 hover:border-gray-600'
                }`}
              >
                <div className="text-sm font-medium text-gray-100">{tmpl.name}</div>
                <div className="text-[11px] text-gray-500 mt-0.5">{tmpl.description}</div>
                {tmpl.defaultTestbedCount > 0 && (
                  <div className="text-[10px] font-mono text-gray-600 mt-1.5">
                    {tmpl.defaultTestbedCount} testbed{tmpl.defaultTestbedCount > 1 ? 's' : ''} / {tmpl.defaultLanguages.length} lang
                  </div>
                )}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* ── Step 1: Testbeds ── */}
      {step === 1 && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-sm font-semibold text-gray-200">Configure Testbeds</h3>
            <button
              onClick={addTestbed}
              className="px-3 py-1.5 rounded border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
            >
              + Add Testbed
            </button>
          </div>

          {testbeds.length === 0 && (
            <div className="border border-dashed border-gray-800 p-4">
              <p className="text-xs text-gray-500 mb-3">No testbeds configured. Add one to define where benchmarks run.</p>
              <div className="flex flex-wrap gap-2">
                <button
                  onClick={() => { const k = testbedKey; setTestbedKey(k + 1); setTestbeds([makeTestbed(k, 'Azure', 'linux')]); }}
                  className="px-3 py-1.5 text-xs font-mono border border-gray-700 text-gray-300 hover:border-cyan-500 hover:text-cyan-300 transition-colors"
                >
                  + Azure / Linux
                </button>
                <button
                  onClick={() => { const k = testbedKey; setTestbedKey(k + 1); setTestbeds([makeTestbed(k, 'Azure', 'windows')]); }}
                  className="px-3 py-1.5 text-xs font-mono border border-gray-700 text-gray-300 hover:border-cyan-500 hover:text-cyan-300 transition-colors"
                >
                  + Azure / Windows
                </button>
                <button
                  onClick={() => { const k = testbedKey; setTestbedKey(k + 1); setTestbeds([makeTestbed(k, 'AWS', 'linux')]); }}
                  className="px-3 py-1.5 text-xs font-mono border border-gray-700 text-gray-300 hover:border-cyan-500 hover:text-cyan-300 transition-colors"
                >
                  + AWS / Linux
                </button>
                <button
                  onClick={() => { const k = testbedKey; setTestbedKey(k + 1); setTestbeds([makeTestbed(k, 'GCP', 'linux')]); }}
                  className="px-3 py-1.5 text-xs font-mono border border-gray-700 text-gray-300 hover:border-cyan-500 hover:text-cyan-300 transition-colors"
                >
                  + GCP / Linux
                </button>
              </div>
            </div>
          )}

          {proxyWarning && (
            <div className="mb-3 border border-yellow-500/30 bg-yellow-500/5 rounded-lg p-3">
              <p className="text-xs text-yellow-300">
                Every testbed must have at least one reverse proxy selected before proceeding.
              </p>
            </div>
          )}

          <div className="space-y-2">
            {testbeds.map((testbed, idx) => (
              <div key={testbed.key} className="border border-gray-800 p-3">
                {/* Row 1: primary axes — Cloud, OS, Region + remove action */}
                <div className="flex items-center gap-2 flex-wrap">
                  <span className="text-[10px] font-mono text-gray-600 w-3">{idx + 1}</span>

                  {/* Cloud — pill buttons */}
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

                  {/* OS — pill buttons */}
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

                  {/* Region — dropdown (many options) */}
                  <select
                    value={testbed.region}
                    onChange={e => updateTestbed(testbed.key, { region: e.target.value })}
                    className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
                  >
                    {(REGIONS[testbed.cloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                  </select>

                  {/* Secondary: topology, size — compact inline */}
                  <select
                    value={testbed.topology}
                    onChange={e => updateTestbed(testbed.key, { topology: e.target.value })}
                    className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs text-gray-500 focus:outline-none focus:border-cyan-500"
                  >
                    {TOPOLOGIES.map(t => <option key={t} value={t}>{t}</option>)}
                  </select>
                  <select
                    value={testbed.vmSize}
                    onChange={e => updateTestbed(testbed.key, { vmSize: e.target.value })}
                    className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs text-gray-500 focus:outline-none focus:border-cyan-500"
                  >
                    {VM_SIZES.map(s => <option key={s} value={s}>{s}</option>)}
                  </select>

                  {/* Existing VM toggle */}
                  <label className="flex items-center gap-1.5 text-[11px] text-gray-500 cursor-pointer ml-auto">
                    <input
                      type="checkbox"
                      checked={testbed.useExisting}
                      onChange={e => updateTestbed(testbed.key, { useExisting: e.target.checked })}
                      className="accent-cyan-400"
                    />
                    existing
                  </label>

                  <button
                    onClick={() => removeTestbed(testbed.key)}
                    className="text-[11px] text-gray-600 hover:text-red-400 transition-colors"
                  >
                    remove
                  </button>
                </div>

                {/* Row 2: Existing VM selector (conditional) */}
                {testbed.useExisting && (
                  <div className="mt-2 ml-5">
                    <select
                      value={testbed.existingVmId}
                      onChange={e => updateTestbed(testbed.key, { existingVmId: e.target.value })}
                      className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
                    >
                      <option value="">Select VM...</option>
                      {catalog
                        .filter(vm => vm.cloud.toLowerCase() === testbed.cloud.toLowerCase() && vm.region.toLowerCase() === testbed.region.toLowerCase())
                        .map(vm => (
                          <option key={vm.vm_id} value={vm.vm_id}>
                            {vm.name} ({vm.ip}) - {vm.status}
                          </option>
                        ))}
                    </select>
                  </div>
                )}

                {/* Proxies */}
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
                        className={`px-2.5 py-1 rounded text-xs border transition-colors ${
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

                {/* Tester OS */}
                <div className="mt-3">
                  <label className="block text-xs text-gray-500 mb-1.5">Tester VM</label>
                  <div className="flex gap-2">
                    {TESTER_OS_OPTIONS.map(opt => (
                      <button
                        key={opt.id}
                        type="button"
                        onClick={() => updateTestbed(testbed.key, { testerOs: opt.id })}
                        className={`px-2.5 py-1 rounded text-xs border transition-colors ${
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
        </div>
      )}

      {/* ── Step 2: Languages ── */}
      {step === 2 && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-sm font-semibold text-gray-200">Select Languages</h3>
            <div className="flex items-center gap-2">
              <button
                onClick={() => setLangShortcut(ALL_LANGUAGE_IDS)}
                className="px-2 py-1 rounded border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
              >
                Select All
              </button>
              <button
                onClick={() => setLangShortcut(TOP_5_IDS)}
                className="px-2 py-1 rounded border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
              >
                Top 5
              </button>
              <button
                onClick={() => setLangShortcut(SYSTEMS_IDS)}
                className="px-2 py-1 rounded border border-gray-700 text-[11px] text-gray-300 hover:border-cyan-500 transition-colors"
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
                        className={`flex items-center gap-2 px-3 py-2 rounded border cursor-pointer transition-colors text-xs ${
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
                          <span className="text-[10px] uppercase tracking-wider text-cyan-500/70 border border-cyan-500/30 rounded px-1 py-0.5">
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
            {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''} selected. nginx is always included as the static baseline.
          </p>

          {requiresWindows(selectedLangs) && testbeds.length === 1 && testbeds[0].os === 'linux' && (
            <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 rounded-lg p-3">
              <p className="text-xs text-yellow-300">
                C# .NET 4.8 requires Windows Server. When you proceed, your testbed will be
                switched to Windows automatically.
              </p>
            </div>
          )}

          {requiresWindows(selectedLangs) && testbeds.length > 1 && testbeds.some(tb => tb.os === 'linux') && (
            <div className="mt-3 border border-yellow-500/30 bg-yellow-500/5 rounded-lg p-3">
              <p className="text-xs text-yellow-300">
                C# .NET 4.8 requires Windows Server. It will only run on testbeds configured with Windows OS.
                Linux testbeds will skip .NET 4.8 automatically.
              </p>
            </div>
          )}
        </div>
      )}

      {/* ── Step 3: Methodology ── */}
      {step === 3 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Methodology</h3>

          {/* Presets */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-6">
            {METHODOLOGY_PRESETS.map(p => (
              <button
                key={p.id}
                onClick={() => applyMethodPreset(p.id)}
                className={`text-left border rounded-lg p-4 transition-colors ${
                  methodPreset === p.id
                    ? 'border-cyan-500/50 bg-cyan-500/5'
                    : 'border-gray-800 bg-[var(--bg-surface)]/40 hover:border-gray-600'
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
            onClick={() => setShowAdvanced(!showAdvanced)}
            className="text-xs text-gray-400 hover:text-gray-200 transition-colors mb-4"
          >
            {showAdvanced ? 'Hide' : 'Show'} advanced options
          </button>

          {showAdvanced && (
            <div className="border border-gray-800 rounded-lg p-4 bg-[var(--bg-surface)]/40 space-y-4">
              <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                <label className="text-xs text-gray-500">
                  Warmup runs
                  <input
                    type="number"
                    value={warmup}
                    onChange={e => { setWarmup(Number(e.target.value)); setMethodPreset('custom'); }}
                    min={0}
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </label>
                <label className="text-xs text-gray-500">
                  Measured runs
                  <input
                    type="number"
                    value={measured}
                    onChange={e => { setMeasured(Number(e.target.value)); setMethodPreset('custom'); }}
                    min={1}
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </label>
                <label className="text-xs text-gray-500">
                  Target error %
                  <input
                    type="number"
                    value={targetError ?? ''}
                    onChange={e => { const v = e.target.value; setTargetError(v === '' ? null : Number(v)); setMethodPreset('custom'); }}
                    min={0}
                    step={0.5}
                    placeholder="None"
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                  />
                </label>
              </div>

              {/* Mode checkboxes */}
              <div>
                <h4 className="text-xs font-medium text-gray-500 mb-2">Modes</h4>
                <div className="flex flex-wrap gap-2">
                  {DEFAULT_MODES.map(mode => (
                    <label
                      key={mode}
                      className={`flex items-center gap-2 px-3 py-2 rounded border cursor-pointer transition-colors text-xs ${
                        selectedModes.has(mode)
                          ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-200'
                          : 'border-gray-800 text-gray-400 hover:border-gray-600'
                      }`}
                    >
                      <input
                        type="checkbox"
                        checked={selectedModes.has(mode)}
                        onChange={() => toggleMode(mode)}
                        className="accent-cyan-400"
                      />
                      {mode}
                    </label>
                  ))}
                </div>
              </div>
            </div>
          )}
        </div>
      )}

      {/* ── Step 4: Review ── */}
      {step === 4 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Review & Launch</h3>

          {/* Benchmark name */}
          <label className="text-xs text-gray-500 block mb-4">
            Benchmark name
            <input
              type="text"
              value={benchmarkName}
              onChange={e => setBenchmarkName(e.target.value)}
              placeholder={`Application benchmark ${new Date().toISOString().slice(0, 10)}`}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </label>

          {/* Summary line */}
          <div className="text-xs font-mono text-gray-400 mb-4">
            {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''} / {totalLanguages} languages / {totalProxies} prox{totalProxies !== 1 ? 'ies' : 'y'} / {totalCombinations} combinations / {totalVMs} new VM{totalVMs !== 1 ? 's' : ''}{totalExisting > 0 ? ` + ${totalExisting} existing` : ''}
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
                  <span className={`text-[10px] px-1 ${
                    testbed.os === 'windows' ? 'text-blue-400' : 'text-green-400'
                  }`}>
                    {testbed.os === 'windows' ? 'win' : 'linux'}
                  </span>
                  <span className="text-gray-600">{testbed.vmSize}</span>
                  <span className="text-gray-700">{testbed.topology}</span>
                  {testbed.useExisting && <span className="text-yellow-600">existing</span>}
                  <span className="text-cyan-500/70">{testbed.proxies.map(p => PROXY_LABELS[p] ?? p).join(', ')}</span>
                  <span className="text-gray-600">{TESTER_OS_OPTIONS.find(o => o.id === testbed.testerOs)?.label ?? testbed.testerOs}</span>
                </div>
              ))}
            </div>
          </div>

          {/* Methodology */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Methodology</div>
            <div className="text-xs font-mono text-gray-400">
              {warmup} warmup / {measured} measured{targetError != null ? ` / ${targetError}% target` : ''} / {Array.from(selectedModes).join(' ')}
            </div>
          </div>

          {/* Languages */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Languages</div>
            <div className="text-xs font-mono text-gray-400">
              {Array.from(selectedLangs).sort().map(lang => {
                const entry = LANGUAGE_GROUPS.flatMap(g => g.entries).find(e => e.id === lang);
                return entry?.label ?? lang;
              }).join(', ')}
            </div>
          </div>

          {/* Auto-teardown */}
          <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer mb-6">
            <input
              type="checkbox"
              checked={autoTeardown}
              onChange={e => setAutoTeardown(e.target.checked)}
              className="accent-cyan-400"
            />
            Auto-teardown VMs after benchmark completes
          </label>

          {submitError && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4 text-red-400 text-sm">
              {submitError}
            </div>
          )}

          <button
            onClick={handleLaunch}
            disabled={submitting || testbeds.length === 0}
            className={`text-white px-6 py-2.5 text-sm font-medium transition-colors ${
              submitting
                ? 'bg-cyan-700 cursor-wait'
                : testbeds.length === 0
                  ? 'bg-gray-700 text-gray-500 cursor-not-allowed'
                  : 'bg-cyan-600 hover:bg-cyan-500'
            }`}
          >
            {submitting ? (
              <span className="flex items-center gap-2">
                <span className="inline-block w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                Launching...
              </span>
            ) : (
              'Launch Benchmark'
            )}
          </button>
        </div>
      )}

      {/* ── Navigation ── */}
      <div className="flex items-center justify-between mt-10 pt-4 border-t border-gray-800/50">
        <button
          onClick={goBack}
          disabled={step === 0}
          className="text-xs text-gray-500 disabled:text-gray-700 disabled:cursor-not-allowed hover:text-gray-300 transition-colors"
        >
          Back
        </button>

        {step < STEP_LABELS.length - 1 && (
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
