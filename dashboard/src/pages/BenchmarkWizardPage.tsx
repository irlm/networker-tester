import { useState, useMemo, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { BenchmarkCellConfig, BenchmarkVmCatalogEntry } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';

// ── Constants ────────────────────────────────────────────────────────────

const STEP_LABELS = ['Template', 'Cells', 'Languages', 'Methodology', 'Review'] as const;

interface TemplateOption {
  id: string;
  name: string;
  description: string;
  defaultCellCount: number;
  defaultLanguages: string[];
  methodology: string;
}

const TEMPLATES: TemplateOption[] = [
  {
    id: 'quick-check',
    name: 'Quick Check',
    description: 'Single loopback cell. Fast validation that the benchmark pipeline works end-to-end.',
    defaultCellCount: 1,
    defaultLanguages: ['nginx', 'rust', 'go'],
    methodology: 'quick',
  },
  {
    id: 'regional-comparison',
    name: 'Regional Comparison',
    description: 'Same cloud, two regions. Measures language performance across geographic distance.',
    defaultCellCount: 2,
    defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'java', 'nodejs'],
    methodology: 'standard',
  },
  {
    id: 'cross-cloud',
    name: 'Cross-Cloud',
    description: 'One cell per cloud provider. Compares language behaviour on Azure, AWS, and GCP.',
    defaultCellCount: 3,
    defaultLanguages: ['nginx', 'rust', 'go', 'csharp-net8', 'java', 'nodejs', 'python'],
    methodology: 'standard',
  },
  {
    id: 'custom',
    name: 'Custom',
    description: 'Start from scratch. Full control over cells, languages, and methodology.',
    defaultCellCount: 0,
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

// ── Cell state ───────────────────────────────────────────────────────────

interface CellState {
  key: number;
  cloud: string;
  region: string;
  topology: string;
  vmSize: string;
  useExisting: boolean;
  existingVmId: string;
}

function makeCell(key: number, cloud?: string): CellState {
  const c = cloud ?? 'Azure';
  return {
    key,
    cloud: c,
    region: REGIONS[c]?.[0] ?? '',
    topology: 'Loopback',
    vmSize: 'Medium',
    useExisting: false,
    existingVmId: '',
  };
}

// ── Component ────────────────────────────────────────────────────────────

export function BenchmarkWizardPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  usePageTitle('New Benchmark');

  const [step, setStep] = useState(0);

  // Step 1: Template
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);

  // Step 2: Cells
  const [cellKey, setCellKey] = useState(0);
  const [cells, setCells] = useState<CellState[]>([]);

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

  const loadCatalog = useCallback(() => {
    if (catalogLoaded || !projectId) return;
    api.listBenchmarkCatalog(projectId)
      .then(data => { setCatalog(data); setCatalogLoaded(true); })
      .catch(() => { setCatalogLoaded(true); });
  }, [projectId, catalogLoaded]);

  // ── Template selection ─────────────────────────────────────────────────

  const applyTemplate = (tmpl: TemplateOption) => {
    setSelectedTemplate(tmpl.id);

    // Pre-fill cells
    const newCells: CellState[] = [];
    if (tmpl.id === 'quick-check') {
      const k = cellKey;
      setCellKey(k + 1);
      newCells.push(makeCell(k, 'Azure'));
    } else if (tmpl.id === 'regional-comparison') {
      let k = cellKey;
      newCells.push(makeCell(k++, 'Azure'));
      const c2 = makeCell(k++, 'Azure');
      c2.region = REGIONS.Azure[1] ?? '';
      newCells.push(c2);
      setCellKey(k);
    } else if (tmpl.id === 'cross-cloud') {
      let k = cellKey;
      newCells.push(makeCell(k++, 'Azure'));
      newCells.push(makeCell(k++, 'AWS'));
      newCells.push(makeCell(k++, 'GCP'));
      setCellKey(k);
    }
    setCells(newCells);

    // Languages
    setSelectedLangs(new Set(tmpl.defaultLanguages));

    // Methodology
    const preset = METHODOLOGY_PRESETS.find(p => p.id === tmpl.methodology) ?? METHODOLOGY_PRESETS[1];
    setMethodPreset(preset.id);
    setWarmup(preset.warmup);
    setMeasured(preset.measured);
    setTargetError(preset.targetError);

    setStep(1);
  };

  // ── Cell helpers ───────────────────────────────────────────────────────

  const addCell = () => {
    const k = cellKey;
    setCellKey(k + 1);
    setCells(prev => [...prev, makeCell(k)]);
  };

  const removeCell = (key: number) => {
    setCells(prev => prev.filter(c => c.key !== key));
  };

  const updateCell = (key: number, patch: Partial<CellState>) => {
    setCells(prev => prev.map(c => {
      if (c.key !== key) return c;
      const updated = { ...c, ...patch };
      // Reset region when cloud changes
      if (patch.cloud && patch.cloud !== c.cloud) {
        updated.region = REGIONS[patch.cloud]?.[0] ?? '';
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
    if (step === 1) return cells.length > 0;
    if (step === 2) return selectedLangs.size > 0;
    if (step === 3) return warmup > 0 && measured > 0 && selectedModes.size > 0;
    return true;
  }, [step, selectedTemplate, cells.length, selectedLangs.size, warmup, measured, selectedModes.size]);

  const goNext = () => {
    if (step === 0 && selectedTemplate === null) return;
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
    const cellConfigs: BenchmarkCellConfig[] = cells.map(c => ({
      cloud: c.cloud,
      region: c.region,
      topology: c.topology,
      vm_size: c.vmSize,
      existing_vm_ip: c.useExisting ? (catalog.find(v => v.vm_id === c.existingVmId)?.ip ?? null) : null,
      languages: Array.from(selectedLangs),
    }));

    return {
      name: benchmarkName.trim() || `Benchmark ${new Date().toISOString().slice(0, 16)}`,
      template: selectedTemplate,
      cells: cellConfigs,
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
      const { config_id } = await api.createBenchmarkConfig(projectId, payload);
      await api.launchBenchmarkConfig(projectId, config_id);
      navigate(`/projects/${projectId}/benchmark-progress/${config_id}`);
    } catch (err) {
      setSubmitError(String(err));
    } finally {
      setSubmitting(false);
    }
  };

  // ── Total estimates ────────────────────────────────────────────────────

  const totalVMs = cells.filter(c => !c.useExisting).length;
  const totalExisting = cells.filter(c => c.useExisting).length;
  const totalLanguages = selectedLangs.size;
  const totalCombinations = cells.length * totalLanguages;

  // ── Render ─────────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      {/* Header */}
      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">New Benchmark</h2>
        <p className="text-xs text-gray-500 mt-1">
          Configure cells, languages, and methodology, then launch.
        </p>
      </div>

      {/* Stepper */}
      <div className="flex items-center gap-1 mb-8">
        {STEP_LABELS.map((label, i) => (
          <div key={label} className="flex items-center gap-1">
            {i > 0 && <div className={`w-6 md:w-10 h-px ${i <= step ? 'bg-cyan-500/60' : 'bg-gray-700'}`} />}
            <button
              onClick={() => { if (i < step) setStep(i); }}
              disabled={i > step}
              className={`flex items-center gap-1.5 px-2 py-1 rounded text-xs font-medium transition-colors ${
                i === step
                  ? 'bg-cyan-500/15 text-cyan-300 border border-cyan-500/40'
                  : i < step
                    ? 'text-gray-400 hover:text-gray-200 border border-transparent'
                    : 'text-gray-600 border border-transparent cursor-not-allowed'
              }`}
            >
              <span className={`w-5 h-5 rounded-full flex items-center justify-center text-[11px] font-bold ${
                i === step ? 'bg-cyan-500/30 text-cyan-200' : i < step ? 'bg-gray-700 text-gray-400' : 'bg-gray-800 text-gray-600'
              }`}>
                {i + 1}
              </span>
              <span className="hidden md:inline">{label}</span>
            </button>
          </div>
        ))}
      </div>

      {/* ── Step 0: Template ── */}
      {step === 0 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Choose a template</h3>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            {TEMPLATES.map(tmpl => (
              <button
                key={tmpl.id}
                onClick={() => applyTemplate(tmpl)}
                className={`text-left border rounded-lg p-4 transition-colors ${
                  selectedTemplate === tmpl.id
                    ? 'border-cyan-500/50 bg-cyan-500/5'
                    : 'border-gray-800 bg-[var(--bg-surface)]/40 hover:border-gray-600'
                }`}
              >
                <h4 className="text-sm font-medium text-gray-100">{tmpl.name}</h4>
                <p className="text-xs text-gray-500 mt-1">{tmpl.description}</p>
                <div className="flex items-center gap-3 mt-3 text-[11px] text-gray-600">
                  {tmpl.defaultCellCount > 0 && <span>{tmpl.defaultCellCount} cell{tmpl.defaultCellCount > 1 ? 's' : ''}</span>}
                  <span>{tmpl.defaultLanguages.length} languages</span>
                  <span>{tmpl.methodology} methodology</span>
                </div>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* ── Step 1: Cells ── */}
      {step === 1 && (
        <div>
          <div className="flex items-center justify-between mb-4">
            <h3 className="text-sm font-semibold text-gray-200">Configure Cells</h3>
            <button
              onClick={addCell}
              className="px-3 py-1.5 rounded border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
            >
              + Add Cell
            </button>
          </div>

          {cells.length === 0 && (
            <div className="border border-dashed border-gray-800 rounded-lg p-8 text-center">
              <p className="text-gray-500 text-sm">No cells yet</p>
              <p className="text-gray-700 text-xs mt-1">Add at least one cell to define where benchmarks will run.</p>
            </div>
          )}

          <div className="space-y-3">
            {cells.map((cell, idx) => (
              <div key={cell.key} className="border border-gray-800 rounded-lg p-4 bg-[var(--bg-surface)]/40">
                <div className="flex items-center justify-between mb-3">
                  <span className="text-xs font-medium text-gray-400">Cell {idx + 1}</span>
                  <button
                    onClick={() => removeCell(cell.key)}
                    className="text-xs text-gray-500 hover:text-red-400 transition-colors"
                  >
                    Remove
                  </button>
                </div>

                <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-3">
                  {/* Cloud */}
                  <label className="text-xs text-gray-500">
                    Cloud
                    <select
                      value={cell.cloud}
                      onChange={e => updateCell(cell.key, { cloud: e.target.value })}
                      className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                    >
                      {CLOUDS.map(c => <option key={c} value={c}>{c}</option>)}
                    </select>
                  </label>

                  {/* Region */}
                  <label className="text-xs text-gray-500">
                    Region
                    <select
                      value={cell.region}
                      onChange={e => updateCell(cell.key, { region: e.target.value })}
                      className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                    >
                      {(REGIONS[cell.cloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                    </select>
                  </label>

                  {/* Topology */}
                  <label className="text-xs text-gray-500">
                    Topology
                    <select
                      value={cell.topology}
                      onChange={e => updateCell(cell.key, { topology: e.target.value })}
                      className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                    >
                      {TOPOLOGIES.map(t => <option key={t} value={t}>{t}</option>)}
                    </select>
                  </label>

                  {/* VM Size */}
                  <label className="text-xs text-gray-500">
                    VM Size
                    <select
                      value={cell.vmSize}
                      onChange={e => updateCell(cell.key, { vmSize: e.target.value })}
                      className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                    >
                      {VM_SIZES.map(s => <option key={s} value={s}>{s}</option>)}
                    </select>
                  </label>
                </div>

                {/* Use existing VM toggle */}
                <div className="mt-3 flex items-center gap-3">
                  <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={cell.useExisting}
                      onChange={e => updateCell(cell.key, { useExisting: e.target.checked })}
                      className="accent-cyan-400"
                    />
                    Use existing VM
                  </label>

                  {cell.useExisting && (
                    <select
                      value={cell.existingVmId}
                      onChange={e => updateCell(cell.key, { existingVmId: e.target.value })}
                      className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-200 focus:outline-none focus:border-cyan-500"
                    >
                      <option value="">Select VM...</option>
                      {catalog
                        .filter(vm => vm.cloud === cell.cloud && vm.region === cell.region)
                        .map(vm => (
                          <option key={vm.vm_id} value={vm.vm_id}>
                            {vm.name} ({vm.ip}) - {vm.status}
                          </option>
                        ))}
                    </select>
                  )}
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

          <div className="space-y-4">
            {LANGUAGE_GROUPS.map(group => (
              <div key={group.label}>
                <h4 className="text-xs font-medium text-gray-500 mb-2">{group.label}</h4>
                <div className="flex flex-wrap gap-2">
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
              placeholder={`Benchmark ${new Date().toISOString().slice(0, 16)}`}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </label>

          {/* Cell summaries */}
          <div className="space-y-2 mb-4">
            {cells.map((cell, idx) => (
              <div key={cell.key} className="border border-gray-800 rounded-lg p-3 bg-[var(--bg-surface)]/40">
                <div className="flex items-center gap-3">
                  <span className="text-xs font-medium text-gray-400">Cell {idx + 1}</span>
                  <span className="text-xs text-gray-300 font-mono">
                    {cell.cloud} / {cell.region}
                  </span>
                  <span className="text-xs text-gray-500">{cell.topology}</span>
                  <span className="text-xs text-gray-500">{cell.vmSize}</span>
                  {cell.useExisting && <span className="text-[10px] text-yellow-500/80">existing VM</span>}
                </div>
              </div>
            ))}
          </div>

          {/* Totals */}
          <div className="border border-gray-800 rounded-lg p-4 bg-[var(--bg-surface)]/40 mb-4">
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4 text-center">
              <div>
                <div className="text-lg font-bold text-gray-100 font-mono">{cells.length}</div>
                <div className="text-xs text-gray-500">Cells</div>
              </div>
              <div>
                <div className="text-lg font-bold text-gray-100 font-mono">{totalLanguages}</div>
                <div className="text-xs text-gray-500">Languages</div>
              </div>
              <div>
                <div className="text-lg font-bold text-gray-100 font-mono">{totalCombinations}</div>
                <div className="text-xs text-gray-500">Combinations</div>
              </div>
              <div>
                <div className="text-lg font-bold text-gray-100 font-mono">{totalVMs}</div>
                <div className="text-xs text-gray-500">New VMs{totalExisting > 0 ? ` + ${totalExisting} existing` : ''}</div>
              </div>
            </div>
          </div>

          {/* Methodology summary */}
          <div className="border border-gray-800 rounded-lg p-4 bg-[var(--bg-surface)]/40 mb-4">
            <h4 className="text-xs font-medium text-gray-500 mb-2">Methodology</h4>
            <div className="flex flex-wrap gap-3 text-xs text-gray-300">
              <span>{warmup} warmup</span>
              <span>{measured} measured</span>
              {targetError != null && <span>{targetError}% error target</span>}
              <span>modes: {Array.from(selectedModes).join(', ')}</span>
            </div>
          </div>

          {/* Languages summary */}
          <div className="border border-gray-800 rounded-lg p-4 bg-[var(--bg-surface)]/40 mb-4">
            <h4 className="text-xs font-medium text-gray-500 mb-2">Languages</h4>
            <div className="flex flex-wrap gap-2">
              {Array.from(selectedLangs).sort().map(lang => {
                const entry = LANGUAGE_GROUPS.flatMap(g => g.entries).find(e => e.id === lang);
                return (
                  <span
                    key={lang}
                    className={`px-2 py-1 rounded border text-xs ${
                      lang === 'nginx'
                        ? 'border-cyan-500/30 text-cyan-300'
                        : 'border-gray-700 text-gray-300'
                    }`}
                  >
                    {entry?.label ?? lang}
                  </span>
                );
              })}
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

          {/* Launch button — the hero moment */}
          <button
            onClick={handleLaunch}
            disabled={submitting || cells.length === 0}
            className={`relative overflow-hidden text-white px-8 py-3 rounded text-sm font-bold tracking-wide transition-all duration-200 ${
              submitting
                ? 'bg-cyan-700 cursor-wait'
                : cells.length === 0
                  ? 'bg-gray-700 text-gray-500 cursor-not-allowed'
                  : 'bg-cyan-600 hover:bg-cyan-500 hover:-translate-y-0.5 hover:shadow-[0_4px_20px_rgba(71,191,255,0.25)] active:translate-y-0 active:shadow-none'
            }`}
          >
            {submitting ? (
              <span className="flex items-center gap-2">
                <span className="inline-block w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                Launching...
              </span>
            ) : (
              <span className="flex items-center gap-2">
                <span className="text-base">{'\u25B6'}</span>
                Launch Benchmark
              </span>
            )}
          </button>
        </div>
      )}

      {/* ── Navigation buttons ── */}
      <div className="flex items-center justify-between mt-8 pt-4 border-t border-gray-800">
        <button
          onClick={goBack}
          disabled={step === 0}
          className="px-4 py-2 rounded border border-gray-700 text-sm text-gray-300 disabled:text-gray-600 disabled:border-gray-800 disabled:cursor-not-allowed hover:border-gray-500 transition-colors"
        >
          Back
        </button>

        {step < STEP_LABELS.length - 1 && (
          <button
            onClick={goNext}
            disabled={!canNext}
            className="px-4 py-2 rounded bg-cyan-600 hover:bg-cyan-500 disabled:bg-gray-700 disabled:text-gray-500 text-white text-sm font-medium transition-colors"
          >
            Next
          </button>
        )}
      </div>
    </div>
  );
}
