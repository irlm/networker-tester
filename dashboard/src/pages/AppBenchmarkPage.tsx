import { useState, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { Workload, Methodology, TestConfigCreate, ComparisonCell, ComparisonGroupCreate } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { WizardStepper } from '../components/wizard/WizardStepper';
import { TestbedMatrix } from '../components/wizard/TestbedMatrix';
import { MethodologyPanel } from '../components/wizard/MethodologyPanel';
import { LanguageSelector } from '../components/wizard/LanguageSelector';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import type { TestbedState } from '../components/wizard/testbed-constants';
import {
  DEFAULT_METHODOLOGY,
  RUNTIME_TEMPLATES,
  PROXY_LABELS,
  TESTER_OS_OPTIONS,
  LANGUAGE_GROUPS,
  WINDOWS_PROXIES,
  requiresWindows,
  makeTestbed,
  resolveVmSize,
  resolveTopology,
  type RuntimeTemplate,
} from '../components/wizard/testbed-constants';

// ── Constants ──────────────────────────────────────────────────────────

const STEPS = ['Template', 'Testbeds', 'Languages', 'Methodology', 'Review'];

// ── Component ──────────────────────────────────────────────────────────

export function AppBenchmarkPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Application Benchmark');

  const [step, setStep] = useState(0);

  // Step 0: Template
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);

  // Step 1: Testbeds
  const [testbeds, setTestbeds] = useState<TestbedState[]>([]);
  const [proxyWarning, setProxyWarning] = useState(false);
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Step 2: Languages
  const [selectedLangs, setSelectedLangs] = useState<Set<string>>(new Set(['nginx']));

  // Workload (configured by template, not a separate step)
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http1', 'http2', 'http3', 'download', 'upload']));
  const runs = 10;
  const concurrency = 1;
  const timeoutMs = 5000;

  // Step 3: Methodology (always on)
  const [methodology, setMethodology] = useState<Methodology>(DEFAULT_METHODOLOGY as Methodology);
  const [methodPreset, setMethodPreset] = useState<string>('standard');

  // Step 4: Review
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [submitting, setSubmitting] = useState(false);

  // ── Template application ────────────────────────────────────────────

  const [testbedKey, setTestbedKey] = useState(0);

  const applyTemplate = useCallback((tmpl: RuntimeTemplate) => {
    setSelectedTemplate(tmpl.id);
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

    // Methodology preset
    const presetMap: Record<string, { warmup: number; measured: number; targetError: number | null }> = {
      quick: { warmup: 5, measured: 10, targetError: null },
      standard: { warmup: 10, measured: 50, targetError: 5 },
      rigorous: { warmup: 10, measured: 200, targetError: 2 },
    };
    const p = presetMap[tmpl.methodology];
    if (p) {
      setMethodPreset(tmpl.methodology);
      setMethodology(m => ({
        ...m,
        warmup_runs: p.warmup,
        measured_runs: p.measured,
        target_error_pct: p.targetError ?? 0,
      }));
    }

    setProxyWarning(false);
    setStep(1);
  }, [testbedKey]);

  // ── Navigation ──────────────────────────────────────────────────────

  const totalProxies = new Set(testbeds.flatMap(tb => tb.proxies)).size;

  const canNext = useMemo(() => {
    if (step === 0) return selectedTemplate !== null;
    if (step === 1) {
      return (
        testbeds.length > 0 &&
        testbeds.every(c => c.proxies.length > 0) &&
        testbeds.every(c => c.cloudAccountId !== '')
      );
    }
    if (step === 2) return selectedLangs.size > 0;
    if (step === 3) return true;
    if (step === 4) return configName.trim().length > 0;
    return true;
  }, [step, selectedTemplate, testbeds, selectedLangs.size, configName]);

  const goNext = () => {
    if (!canNext || step >= 4) return;
    if (step === 1) {
      const missingProxies = testbeds.some(tb => tb.proxies.length === 0);
      if (missingProxies) { setProxyWarning(true); return; }
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
    setStep(step + 1);
  };

  const goBack = () => {
    if (step > 0) setStep(step - 1);
  };

  // ── Submit ──────────────────────────────────────────────────────────

  // Fan out across (testbed × proxy × language). One deployment will be
  // created per unique (cloud_account_id, region, vm_size, os) — the
  // orchestrator dedups and stacks languages/proxies into its http_stacks
  // array when install.sh runs.
  const buildComparisonCells = (): ComparisonCell[] => {
    const cells: ComparisonCell[] = [];
    const langs = selectedLangs.size > 0 ? [...selectedLangs] : [''];
    for (const lang of langs) {
      for (const tb of testbeds) {
        const vmSize = resolveVmSize(tb.cloud, tb.vmSize);
        const topology = resolveTopology(tb.topology);
        for (const proxy of tb.proxies) {
          const label = [lang, `${tb.cloud}/${tb.region}`, tb.os, proxy]
            .filter(Boolean)
            .join(' @ ');
          cells.push({
            label,
            endpoint: {
              kind: 'pending',
              cloud_account_id: tb.cloudAccountId,
              region: tb.region,
              vm_size: vmSize,
              os: tb.os,
              proxy_stack: proxy,
              topology,
              ...(lang ? { language: lang } : {}),
            },
            ...(selectedTesterId ? { runner_id: selectedTesterId } : {}),
          });
        }
      }
    }
    return cells;
  };

  const isMatrixRun = buildComparisonCells().length > 1;

  const handleSubmit = async (launchNow: boolean) => {
    setSubmitting(true);
    try {
      const workload: Workload = {
        modes: [...selectedModes],
        runs,
        concurrency,
        timeout_ms: timeoutMs,
        payload_sizes: [],
        capture_mode: 'headers-only',
      };

      if (isMatrixRun && launchNow) {
        const cells = buildComparisonCells();
        const body: ComparisonGroupCreate = {
          name: configName,
          base_workload: workload,
          methodology,
          cells,
        };
        const group = await api.createComparisonGroup(projectId, body);
        await api.launchComparisonGroup(group.id);
        addToast('success', `Launched ${cells.length} run${cells.length === 1 ? '' : 's'}`);
        navigate(`/projects/${projectId}/runs?comparison_group=${group.id}`);
        return;
      }

      // Single-cell path: one Pending endpoint, single deployment.
      const cells = buildComparisonCells();
      const onlyEndpoint = cells[0]?.endpoint;
      if (!onlyEndpoint) {
        addToast('error', 'At least one testbed, proxy, and language are required');
        return;
      }
      const config: TestConfigCreate = {
        name: configName,
        endpoint: onlyEndpoint,
        workload,
        methodology,
      };

      const created = await api.createTestConfig(projectId, config);

      if (addSchedule) {
        await api.createTestSchedule(projectId, {
          test_config_id: created.id,
          cron_expr: cronExpr,
        });
      }

      if (launchNow) {
        const run = await api.launchTestConfig(created.id, selectedTesterId ?? undefined);
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

  // ── Render ──────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <Breadcrumb items={[{ label: 'Application', to: `/projects/${projectId}/runs` }, { label: 'New Benchmark' }]} />

      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">New Application Benchmark</h2>
        <p className="text-xs text-gray-500 mt-1">
          Compare language and framework performance with statistical methodology.
        </p>
      </div>

      <WizardStepper steps={STEPS} currentStep={step} onStepClick={setStep} />

      {/* ── Step 0: Template ── */}
      {step === 0 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Choose a template</h3>
          <div className="grid grid-cols-2 md:grid-cols-3 gap-2">
            {RUNTIME_TEMPLATES.map(tmpl => (
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
                    {tmpl.defaultTestbedCount} testbed / {tmpl.defaultLanguages.length} lang
                  </div>
                )}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* ── Step 1: Testbeds ── */}
      {step === 1 && (
        <TestbedMatrix
          projectId={projectId}
          testbeds={testbeds}
          onTestbedsChange={setTestbeds}
          runnerMode={runnerMode}
          onRunnerModeChange={setRunnerMode}
          selectedTesterId={selectedTesterId}
          onTesterIdChange={setSelectedTesterId}
          proxyWarning={proxyWarning}
        />
      )}

      {/* ── Step 2: Languages ── */}
      {step === 2 && (
        <LanguageSelector
          selectedLangs={selectedLangs}
          onLangsChange={setSelectedLangs}
          testbeds={testbeds}
        />
      )}

      {/* ── Step 3: Methodology ── */}
      {step === 3 && (
        <MethodologyPanel
          alwaysOn
          benchmarkMode
          onBenchmarkModeChange={() => {}}
          methodology={methodology}
          onMethodologyChange={setMethodology}
          methodPreset={methodPreset}
          onMethodPresetChange={setMethodPreset}
        />
      )}

      {/* ── Step 4: Review ── */}
      {step === 4 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Review & Launch</h3>

          <label className="text-xs text-gray-500 block mb-4">
            Benchmark name
            <input
              type="text"
              value={configName}
              onChange={e => setConfigName(e.target.value)}
              placeholder={`Application benchmark ${new Date().toISOString().slice(0, 10)}`}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </label>

          {/* Summary */}
          <div className="text-xs font-mono text-gray-400 mb-4">
            {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
            {' / '}{selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''}
            {' / '}{totalProxies} prox{totalProxies !== 1 ? 'ies' : 'y'}
            {' / '}{[...selectedModes].join(', ')}
          </div>

          {/* Template */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Template</div>
            <div className="text-xs font-mono text-gray-400">
              {RUNTIME_TEMPLATES.find(t => t.id === selectedTemplate)?.name ?? selectedTemplate}
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

          {/* Languages */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Languages</div>
            <div className="text-xs font-mono text-gray-400">
              {[...selectedLangs].sort().map(lang => {
                const entry = LANGUAGE_GROUPS.flatMap(g => g.entries).find(e => e.id === lang);
                return entry?.label ?? lang;
              }).join(', ')}
            </div>
          </div>

          {/* Methodology */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Methodology</div>
            <div className="text-xs font-mono text-gray-400">
              {methodology.warmup_runs} warmup / {methodology.measured_runs} measured / {methodology.target_error_pct > 0 ? `${methodology.target_error_pct}% target error` : 'no error target'}
            </div>
          </div>

          {/* Workload */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Workload</div>
            <div className="text-xs font-mono text-gray-400">
              {runs} runs x {concurrency} concurrency / {timeoutMs}ms timeout / {[...selectedModes].join(' ')}
            </div>
          </div>

          {isMatrixRun && (
            <div className="text-xs font-mono text-purple-400 mb-4">
              Comparison group: {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''} x {selectedLangs.size} language{selectedLangs.size !== 1 ? 's' : ''} = {buildComparisonCells().length} runs
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
                submitting ? 'bg-cyan-700 cursor-wait' : 'bg-cyan-600 hover:bg-cyan-500'
              }`}
            >
              {submitting ? (
                <span className="flex items-center gap-2">
                  <span className="inline-block w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  Launching...
                </span>
              ) : isMatrixRun ? `Launch ${buildComparisonCells().length} Runs` : 'Launch Now'}
            </button>
          </div>
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
        {step > 0 && step < 4 && (
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
