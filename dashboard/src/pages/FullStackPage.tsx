import { useState, useEffect, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { Workload, Methodology, TestConfigCreate, ModeGroup, ComparisonCell, ComparisonGroupCreate } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { WizardStepper } from '../components/wizard/WizardStepper';
import { TestbedMatrix } from '../components/wizard/TestbedMatrix';
import { WorkloadPanel } from '../components/wizard/WorkloadPanel';
import { MethodologyPanel } from '../components/wizard/MethodologyPanel';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import type { TestbedState } from '../components/wizard/testbed-constants';
import { DEFAULT_METHODOLOGY, PROXY_LABELS, TESTER_OS_OPTIONS } from '../components/wizard/testbed-constants';

// ── Constants ──────────────────────────────────────────────────────────

const STEPS = ['Testbeds', 'Workload', 'Methodology', 'Review'];

// ── Component ──────────────────────────────────────────────────────────

export function FullStackPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Full Stack Benchmark');

  const [step, setStep] = useState(0);

  // Step 0: Testbeds
  const [testbeds, setTestbeds] = useState<TestbedState[]>([]);
  const [proxyWarning, setProxyWarning] = useState(false);
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Step 1: Workload
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http1', 'http2', 'http3', 'download', 'upload']));
  const [runs, setRuns] = useState(10);
  const [concurrency, setConcurrency] = useState(1);
  const [timeoutMs, setTimeoutMs] = useState(5000);
  const [selectedPayloads, setSelectedPayloads] = useState<Set<string>>(new Set());
  const [insecure, setInsecure] = useState(false);
  const [connectionReuse, setConnectionReuse] = useState(true);
  const [captureMode, setCaptureMode] = useState<'none' | 'tester' | 'endpoint' | 'both'>('none');

  // Step 2: Methodology (always on)
  const [methodology, setMethodology] = useState<Methodology>(DEFAULT_METHODOLOGY as Methodology);
  const [methodPreset, setMethodPreset] = useState<string>('standard');

  // Step 3: Review
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [submitting, setSubmitting] = useState(false);

  // ── Data loading ────────────────────────────────────────────────────

  useEffect(() => {
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
  }, []);

  // ── Navigation ──────────────────────────────────────────────────────

  const totalProxies = new Set(testbeds.flatMap(tb => tb.proxies)).size;

  const canNext = useMemo(() => {
    if (step === 0) return testbeds.length > 0 && testbeds.every(c => c.proxies.length > 0);
    if (step === 1) return selectedModes.size > 0;
    if (step === 2) return true;
    if (step === 3) return configName.trim().length > 0;
    return true;
  }, [step, testbeds, selectedModes.size, configName]);

  const goNext = () => {
    if (!canNext || step >= 3) return;
    if (step === 0) {
      const missingProxies = testbeds.some(tb => tb.proxies.length === 0);
      if (missingProxies) { setProxyWarning(true); return; }
      setProxyWarning(false);
    }
    setStep(step + 1);
  };

  const goBack = () => {
    if (step > 0) setStep(step - 1);
  };

  // ── Submit ──────────────────────────────────────────────────────────

  const buildComparisonCells = (): ComparisonCell[] => {
    return testbeds.map(tb => ({
      label: `${tb.cloud}/${tb.region} ${tb.os}`,
      endpoint: { kind: 'proxy' as const, proxy_endpoint_id: '' },
      ...(selectedTesterId ? { runner_id: selectedTesterId } : {}),
    }));
  };

  const isMatrixRun = testbeds.length > 1;

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
          methodology,
          cells,
        };
        const group = await api.createComparisonGroup(projectId, body);
        addToast('success', `Comparison group launched -- ${cells.length} runs`);
        navigate(`/projects/${projectId}/runs?comparison_group=${group.id}`);
        return;
      }

      const config: TestConfigCreate = {
        name: configName,
        endpoint: { kind: 'proxy', proxy_endpoint_id: '' },
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
      <Breadcrumb items={[{ label: 'Full Stack', to: `/projects/${projectId}/runs` }, { label: 'New Benchmark' }]} />

      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">New Full Stack Benchmark</h2>
        <p className="text-xs text-gray-500 mt-1">
          Test infrastructure stack performance through proxies with statistical methodology.
        </p>
      </div>

      <WizardStepper steps={STEPS} currentStep={step} onStepClick={setStep} />

      {/* ── Step 0: Testbeds ── */}
      {step === 0 && (
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

      {/* ── Step 1: Workload ── */}
      {step === 1 && (
        <WorkloadPanel
          modeGroups={modeGroups}
          selectedModes={selectedModes}
          onModesChange={setSelectedModes}
          runs={runs}
          onRunsChange={setRuns}
          concurrency={concurrency}
          onConcurrencyChange={setConcurrency}
          timeoutMs={timeoutMs}
          onTimeoutChange={setTimeoutMs}
          selectedPayloads={selectedPayloads}
          onPayloadsChange={setSelectedPayloads}
          insecure={insecure}
          onInsecureChange={setInsecure}
          connectionReuse={connectionReuse}
          onConnectionReuseChange={setConnectionReuse}
          captureMode={captureMode}
          onCaptureModeChange={setCaptureMode}
        />
      )}

      {/* ── Step 2: Methodology ── */}
      {step === 2 && (
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

      {/* ── Step 3: Review ── */}
      {step === 3 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Review & Launch</h3>

          <label className="text-xs text-gray-500 block mb-4">
            Benchmark name
            <input
              type="text"
              value={configName}
              onChange={e => setConfigName(e.target.value)}
              placeholder={`Full stack benchmark ${new Date().toISOString().slice(0, 10)}`}
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </label>

          {/* Summary */}
          <div className="text-xs font-mono text-gray-400 mb-4">
            {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
            {' / '}{totalProxies} prox{totalProxies !== 1 ? 'ies' : 'y'}
            {' / '}{[...selectedModes].join(', ')}
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

          {/* Methodology */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Methodology</div>
            <div className="text-xs font-mono text-gray-400">
              {methodology.warmup_runs} warmup / {methodology.measured_runs} measured / {methodology.target_error_pct}% target error
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
              Comparison group: {testbeds.length} testbed{testbeds.length !== 1 ? 's' : ''}
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
            <button
              onClick={() => handleSubmit(false)}
              disabled={submitting || configName.trim().length === 0}
              className="border border-gray-700 hover:border-gray-600 text-gray-300 px-4 py-2 text-sm transition-colors disabled:opacity-40"
            >
              Save Config
            </button>
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
              ) : isMatrixRun ? `Launch ${testbeds.length} Runs` : 'Launch Now'}
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
        {step < 3 && (
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
