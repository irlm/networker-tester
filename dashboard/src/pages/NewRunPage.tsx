import { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { EndpointRef, EndpointKind, Workload, Methodology, TestConfigCreate, ModeGroup } from '../api/types';
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

export function NewRunPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Run');

  // Step tracking
  const [step, setStep] = useState<Step>(1);

  // Step 1: Endpoint
  const [endpointKind, setEndpointKind] = useState<EndpointKind>('network');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('');
  const [proxyEndpointId, setProxyEndpointId] = useState('');
  const [runtimeId, setRuntimeId] = useState('');
  const [language, setLanguage] = useState('');

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
            {s === 1 ? 'Endpoint' : s === 2 ? 'Workload' : s === 3 ? 'Methodology' : 'Save & Launch'}
          </button>
        ))}
      </div>

      {/* Step 1: Endpoint kind */}
      {step === 1 && (
        <div className="space-y-4">
          <div>
            <label className="text-xs text-gray-500 mb-2 block">Endpoint Type</label>
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

          {endpointKind === 'proxy' && (
            <div>
              <label htmlFor="proxyId" className="text-xs text-gray-500 mb-1 block">Proxy Endpoint ID</label>
              <input
                id="proxyId"
                type="text"
                value={proxyEndpointId}
                onChange={e => setProxyEndpointId(e.target.value)}
                placeholder="UUID of deployed endpoint"
                className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
              />
            </div>
          )}

          {endpointKind === 'runtime' && (
            <div className="space-y-3">
              <div>
                <label htmlFor="runtimeId" className="text-xs text-gray-500 mb-1 block">Runtime ID</label>
                <input
                  id="runtimeId"
                  type="text"
                  value={runtimeId}
                  onChange={e => setRuntimeId(e.target.value)}
                  placeholder="UUID from Runtimes catalog"
                  className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                />
              </div>
              <div>
                <label htmlFor="language" className="text-xs text-gray-500 mb-1 block">Language</label>
                <input
                  id="language"
                  type="text"
                  value={language}
                  onChange={e => setLanguage(e.target.value)}
                  placeholder="e.g. go, rust, node"
                  className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 w-48 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                />
              </div>
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
            <p><span className="text-gray-500">Endpoint:</span> {endpointKind}{endpointKind === 'network' ? ` / ${host}` : ''}</p>
            <p><span className="text-gray-500">Modes:</span> {[...selectedModes].join(', ')}</p>
            <p><span className="text-gray-500">Iterations:</span> {runs} x {concurrency} concurrency</p>
            {benchmarkMode && <p><span className="text-purple-400">Benchmark mode enabled</span> -- {methodology.measured_runs} measured runs</p>}
          </div>

          <div className="flex gap-2 pt-4">
            <button onClick={() => setStep(3)} className="text-gray-400 hover:text-gray-200 px-4 py-2 text-sm transition-colors">
              Back
            </button>
            <button
              onClick={() => handleSubmit(false)}
              disabled={submitting || !canAdvance(4)}
              className="border border-gray-700 hover:border-gray-600 text-gray-300 px-4 py-2 rounded text-sm transition-colors disabled:opacity-40"
            >
              Save Config
            </button>
            <button
              onClick={() => handleSubmit(true)}
              disabled={submitting || !canAdvance(4)}
              className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-4 py-2 rounded text-sm transition-colors"
            >
              {submitting ? 'Launching...' : 'Launch Now'}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
