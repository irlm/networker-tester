import { useState, useEffect, useRef, useCallback } from 'react';
import { api } from '../api/client';
import type { ModeGroup, Deployment, Agent, BenchmarkConfigSummary } from '../api/types';
import { THROUGHPUT_IDS } from '../lib/chart';
import { ModeSelector } from './common/ModeSelector';
import { PayloadSelector } from './common/PayloadSelector';
import { useToast } from '../hooks/useToast';

interface CreateScheduleDialogProps {
  projectId: string;
  onClose: () => void;
  onCreated: () => void;
}

const FREQUENCY_PRESETS = [
  { label: 'Every 15 min', cron: '0 */15 * * * *' },
  { label: 'Hourly', cron: '0 0 * * * *' },
  { label: 'Every 6 hours', cron: '0 0 */6 * * *' },
  { label: 'Daily (midnight)', cron: '0 0 0 * * *' },
  { label: 'Daily (9 AM)', cron: '0 0 9 * * *' },
  { label: 'Weekly (Monday)', cron: '0 0 0 * * 1' },
];

export function CreateScheduleDialog({ projectId, onClose, onCreated }: CreateScheduleDialogProps) {
  // Step tracking
  const [step, setStep] = useState(1);

  // Schedule type: 'test' or 'benchmark'
  const [scheduleType, setScheduleType] = useState<'test' | 'benchmark'>('test');

  // Benchmark config template selection
  const [benchmarkConfigs, setBenchmarkConfigs] = useState<BenchmarkConfigSummary[]>([]);
  const [selectedBenchmarkConfigId, setSelectedBenchmarkConfigId] = useState('');

  // Schedule metadata
  const [name, setName] = useState('');
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [customCron, setCustomCron] = useState('');
  const [useCustomCron, setUseCustomCron] = useState(false);

  // Target
  const [target, setTarget] = useState('https://localhost:8443/health');
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [selectedDeploymentId, setSelectedDeploymentId] = useState<string>('');
  const [endpointHealth, setEndpointHealth] = useState<Record<string, boolean | undefined>>({});

  // Test config
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http1', 'http2']));
  const [runs, setRuns] = useState(3);
  const [concurrency, setConcurrency] = useState(1);
  const [timeout, setTimeout_] = useState(30);
  const [insecure, setInsecure] = useState(true);
  const [connectionReuse, setConnectionReuse] = useState(false);
  const [payloadSizes, setPayloadSizes] = useState<Set<string>>(new Set(['64k', '1m']));
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);

  // Tester
  const [testers, setTesters] = useState<Agent[]>([]);
  const [selectedTester, setSelectedTester] = useState('');

  // VM options
  const [autoStartVm, setAutoStartVm] = useState(false);
  const [autoStopVm, setAutoStopVm] = useState(false);

  // UI state
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const addToast = useToast();

  const needsPayload = THROUGHPUT_IDS.some((m) => selectedModes.has(m));

  useEffect(() => {
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
    api.listBenchmarkConfigs(projectId).then(configs => {
      // Only show completed configs as templates
      setBenchmarkConfigs(configs.filter(c => c.status === 'completed' || c.status === 'draft'));
    }).catch(() => {});
    api.getDeployments(projectId, { limit: 20 }).then(deps => {
      const completed = deps.filter(d => d.status === 'completed' && d.endpoint_ips && d.endpoint_ips.length > 0);
      setDeployments(completed);
      completed.forEach(d => {
        setEndpointHealth(prev => ({ ...prev, [d.deployment_id]: undefined }));
        api.checkDeployment(projectId, d.deployment_id)
          .then((result: { endpoints: { ip: string; alive: boolean }[] }) => {
            const anyAlive = result.endpoints.some(ep => ep.alive);
            setEndpointHealth(prev => ({ ...prev, [d.deployment_id]: anyAlive }));
          })
          .catch(() => {
            setEndpointHealth(prev => ({ ...prev, [d.deployment_id]: false }));
          });
      });
    }).catch(() => {});
    api.getAgents(projectId).then(r => setTesters(r.agents)).catch(() => {});
  }, [projectId]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); },
    [onClose]
  );

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  const toggleMode = (id: string) => {
    setSelectedModes((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const togglePayload = (val: string) => {
    setPayloadSizes((prev) => {
      const next = new Set(prev);
      if (next.has(val)) next.delete(val); else next.add(val);
      return next;
    });
  };

  const handleSubmit = async () => {
    if (!name.trim()) { setError('Name is required'); return; }

    if (scheduleType === 'test' && selectedModes.size === 0) {
      setError('Select at least one mode');
      return;
    }
    if (scheduleType === 'benchmark' && !selectedBenchmarkConfigId) {
      setError('Select a benchmark config template');
      return;
    }

    const finalCron = useCustomCron ? customCron : cronExpr;

    setLoading(true);
    setError(null);
    try {
      const normalizedTarget = target.match(/^https?:\/\//) ? target : `https://${target}`;
      const result = await api.createSchedule(projectId, {
        name: name.trim(),
        cron_expr: finalCron,
        config: scheduleType === 'test' ? {
          target: normalizedTarget,
          modes: Array.from(selectedModes),
          runs,
          concurrency,
          timeout_secs: timeout,
          payload_sizes: needsPayload ? Array.from(payloadSizes) : [],
          insecure,
          dns_enabled: true,
          connection_reuse: connectionReuse,
        } : {},
        agent_id: selectedTester || undefined,
        deployment_id: selectedDeploymentId || undefined,
        auto_start_vm: autoStartVm,
        auto_stop_vm: autoStopVm,
        benchmark_config_id: scheduleType === 'benchmark' ? selectedBenchmarkConfigId : undefined,
      } as Parameters<typeof api.createSchedule>[1]);
      addToast('success', `Schedule created — next run ${new Date(result.next_run_at).toLocaleString()}`);
      onCreated();
      onClose();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to create schedule';
      setError(msg);
      addToast('error', msg);
    } finally {
      setLoading(false);
    }
  };

  const titleId = 'create-schedule-dialog-title';
  // Benchmark schedules skip step 2 (test config) — only 3 steps
  const totalSteps = scheduleType === 'benchmark' ? 3 : 4;

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />

      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative w-full md:w-[560px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-2">
            <h3 id={titleId} className="text-lg font-bold text-gray-100">
              New Schedule
            </h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">&#x2715;</button>
          </div>

          {/* Step indicator */}
          <div className="flex gap-1 mb-6">
            {Array.from({ length: totalSteps }, (_, i) => (
              <div
                key={i}
                className={`h-1 flex-1 rounded-full transition-colors ${
                  i + 1 <= step ? 'bg-cyan-500' : 'bg-gray-800'
                }`}
              />
            ))}
          </div>

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
              <p className="text-red-400 text-sm">{error}</p>
            </div>
          )}

          {/* Step 1: Name & Target */}
          {step === 1 && (
            <div>
              <p className="text-xs text-gray-500 mb-4">Step 1 — Name & Type</p>

              {/* Schedule type toggle */}
              <div className="flex gap-2 mb-4">
                <button
                  type="button"
                  onClick={() => setScheduleType('test')}
                  className={`flex-1 px-3 py-2 text-sm rounded border transition-colors ${
                    scheduleType === 'test'
                      ? 'border-cyan-500 text-cyan-400 bg-cyan-500/10'
                      : 'border-gray-700 text-gray-400 hover:border-cyan-500/50'
                  }`}
                >
                  Test
                </button>
                <button
                  type="button"
                  onClick={() => setScheduleType('benchmark')}
                  className={`flex-1 px-3 py-2 text-sm rounded border transition-colors ${
                    scheduleType === 'benchmark'
                      ? 'border-cyan-500 text-cyan-400 bg-cyan-500/10'
                      : 'border-gray-700 text-gray-400 hover:border-cyan-500/50'
                  }`}
                >
                  Benchmark
                </button>
              </div>

              <label className="block text-xs text-gray-400 mb-1">Schedule Name</label>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={scheduleType === 'test' ? 'e.g., Hourly Azure check' : 'e.g., Nightly latency benchmark'}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500"
                autoFocus
              />

              {/* Benchmark config selector */}
              {scheduleType === 'benchmark' && (
                <div className="mb-4">
                  <label className="block text-xs text-gray-400 mb-1">Benchmark Config Template</label>
                  <select
                    value={selectedBenchmarkConfigId}
                    onChange={(e) => setSelectedBenchmarkConfigId(e.target.value)}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  >
                    <option value="">Select a benchmark config...</option>
                    {benchmarkConfigs.map(c => (
                      <option key={c.config_id} value={c.config_id}>
                        {c.name} ({c.status})
                      </option>
                    ))}
                  </select>
                  <p className="text-xs text-gray-600 mt-1">
                    The selected config will be cloned each time the schedule triggers.
                  </p>
                </div>
              )}

              {scheduleType === 'test' && (<>
              <label className="block text-xs text-gray-400 mb-1">Target Endpoint</label>
              <select
                value={target}
                onChange={(e) => {
                  setTarget(e.target.value);
                  // Auto-set deployment_id from selected endpoint
                  const dep = deployments.find(d =>
                    (d.endpoint_ips || []).some(ip => e.target.value.includes(ip))
                  );
                  setSelectedDeploymentId(dep?.deployment_id || '');
                }}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-2 focus:outline-none focus:border-cyan-500"
              >
                <option value="">Select endpoint...</option>
                {deployments.flatMap(d => {
                  const health = endpointHealth[d.deployment_id];
                  const status = health === undefined ? '...' : health ? '\u2714' : '\u2716 offline';
                  return (d.endpoint_ips || []).map(ip => (
                    <option key={`${d.deployment_id}-${ip}`} value={`https://${ip}:8443/health`}>
                      {d.name} [{status}]
                    </option>
                  ));
                })}
              </select>
              <input
                value={target}
                onChange={(e) => setTarget(e.target.value)}
                placeholder="Or type a custom URL..."
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-400 mb-4 focus:outline-none focus:border-cyan-500"
              />

              {testers.length > 0 && (
                <div className="mb-4">
                  <label className="block text-xs text-gray-400 mb-1">Tester</label>
                  <select
                    value={selectedTester}
                    onChange={(e) => setSelectedTester(e.target.value)}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  >
                    <option value="">Auto (any online tester)</option>
                    {testers.map(a => (
                      <option key={a.agent_id} value={a.agent_id}>
                        {a.name} ({a.status}){a.region ? ` \u2014 ${a.region}` : ''}
                      </option>
                    ))}
                  </select>
                </div>
              )}
              </>)}
            </div>
          )}

          {/* Step 2: Test Configuration (test type only; benchmark skips) */}
          {step === 2 && scheduleType === 'test' && (
            <div>
              <p className="text-xs text-gray-500 mb-4">Step 2 — Test Configuration</p>

              <p className="text-xs text-gray-400 mb-2">Probe Modes</p>
              <div className="mb-4">
                <ModeSelector
                  modeGroups={modeGroups}
                  selectedModes={selectedModes}
                  onToggle={toggleMode}
                  onToggleGroup={(ids, allSelected) => {
                    setSelectedModes(prev => {
                      const next = new Set(prev);
                      ids.forEach(id => allSelected ? next.delete(id) : next.add(id));
                      return next;
                    });
                  }}
                />
              </div>

              {needsPayload && (
                <div className="mb-4">
                  <p className="text-xs text-gray-400 mb-2">Payload Sizes</p>
                  <PayloadSelector selected={payloadSizes} onToggle={togglePayload} />
                </div>
              )}

              <div className="grid grid-cols-3 gap-3 mb-4">
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Runs</label>
                  <input type="number" min={1} max={100} value={runs}
                    onChange={(e) => setRuns(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Concurrency</label>
                  <input type="number" min={1} max={50} value={concurrency}
                    onChange={(e) => setConcurrency(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Timeout (sec)</label>
                  <input type="number" min={1} max={300} value={timeout}
                    onChange={(e) => setTimeout_(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              </div>

              <div className="flex gap-6 mb-4">
                <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
                  <input type="checkbox" checked={insecure} onChange={(e) => setInsecure(e.target.checked)} className="accent-cyan-500" />
                  Skip TLS verify
                </label>
                <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
                  <input type="checkbox" checked={connectionReuse} onChange={(e) => setConnectionReuse(e.target.checked)} className="accent-cyan-500" />
                  Connection reuse
                </label>
              </div>
            </div>
          )}

          {/* Step 3 (test) / Step 2 (benchmark): Frequency */}
          {((scheduleType === 'test' && step === 3) || (scheduleType === 'benchmark' && step === 2)) && (
            <div>
              <p className="text-xs text-gray-500 mb-4">Step {scheduleType === 'benchmark' ? 2 : 3} — Frequency</p>

              <div className="grid grid-cols-2 gap-2 mb-4">
                {FREQUENCY_PRESETS.map((p) => (
                  <button
                    key={p.cron}
                    type="button"
                    onClick={() => { setCronExpr(p.cron); setUseCustomCron(false); }}
                    className={`px-3 py-2 text-sm rounded border transition-colors ${
                      !useCustomCron && cronExpr === p.cron
                        ? 'border-cyan-500 text-cyan-400 bg-cyan-500/10'
                        : 'border-gray-700 text-gray-400 hover:border-cyan-500/50'
                    }`}
                  >
                    {p.label}
                  </button>
                ))}
              </div>

              <label className="flex items-center gap-2 text-sm text-gray-400 mb-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={useCustomCron}
                  onChange={(e) => setUseCustomCron(e.target.checked)}
                  className="accent-cyan-500"
                />
                Custom cron expression
              </label>
              {useCustomCron && (
                <div>
                  <input
                    value={customCron}
                    onChange={(e) => setCustomCron(e.target.value)}
                    placeholder="sec min hour day month weekday (e.g., 0 */5 * * * *)"
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-1 font-mono focus:outline-none focus:border-cyan-500"
                  />
                  <p className="text-xs text-gray-600">6-field cron: second minute hour day-of-month month day-of-week</p>
                </div>
              )}
            </div>
          )}

          {/* Step 4 (test) / Step 3 (benchmark): VM Options & Review */}
          {((scheduleType === 'test' && step === 4) || (scheduleType === 'benchmark' && step === 3)) && (
            <div>
              <p className="text-xs text-gray-500 mb-4">Step {scheduleType === 'benchmark' ? 3 : 4} — VM Options & Review</p>

              {selectedDeploymentId && (
                <div className="mb-4 border border-gray-800 rounded p-3">
                  <p className="text-xs text-gray-400 mb-2">VM Lifecycle (cost saving)</p>
                  <label className="flex items-center gap-2 text-sm text-gray-300 mb-2 cursor-pointer">
                    <input type="checkbox" checked={autoStartVm} onChange={(e) => setAutoStartVm(e.target.checked)} className="accent-cyan-500" />
                    Auto-start VM before test
                  </label>
                  <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                    <input type="checkbox" checked={autoStopVm} onChange={(e) => setAutoStopVm(e.target.checked)} className="accent-cyan-500" />
                    Auto-stop VM after test completes
                  </label>
                  {(autoStartVm || autoStopVm) && (
                    <p className="text-xs text-yellow-400/70 mt-2">
                      Uses cloud CLI (az/aws/gcloud) to manage VM power state
                    </p>
                  )}
                </div>
              )}

              {!selectedDeploymentId && (
                <div className="mb-4 border border-gray-800 rounded p-3">
                  <p className="text-xs text-gray-600">VM auto-start/stop requires a deployed endpoint. Select a deployment in step 1.</p>
                </div>
              )}

              {/* Review summary */}
              <div className="border border-gray-800 rounded p-4 text-sm">
                <p className="text-gray-400 text-xs tracking-wider mb-3">review</p>
                <div className="space-y-2">
                  <div className="flex justify-between">
                    <span className="text-gray-500">Name</span>
                    <span className="text-gray-200">{name || '—'}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-gray-500">Type</span>
                    <span className="text-gray-200">{scheduleType === 'benchmark' ? 'Benchmark' : 'Test'}</span>
                  </div>
                  {scheduleType === 'benchmark' && (
                    <div className="flex justify-between">
                      <span className="text-gray-500">Template</span>
                      <span className="text-gray-200 text-xs truncate max-w-[250px]">
                        {benchmarkConfigs.find(c => c.config_id === selectedBenchmarkConfigId)?.name || '—'}
                      </span>
                    </div>
                  )}
                  {scheduleType === 'test' && (
                    <>
                      <div className="flex justify-between">
                        <span className="text-gray-500">Target</span>
                        <span className="text-gray-200 text-xs font-mono truncate max-w-[250px]">
                          {target.replace('https://', '').replace('/health', '')}
                        </span>
                      </div>
                      <div className="flex justify-between items-start">
                        <span className="text-gray-500">Modes</span>
                        <span className="text-gray-200 text-xs text-right max-w-[280px]">
                          {Array.from(selectedModes).join(', ')}
                        </span>
                      </div>
                    </>
                  )}
                  <div className="flex justify-between">
                    <span className="text-gray-500">Frequency</span>
                    <span className="text-gray-200 text-xs">
                      {(() => {
                        const finalCron = useCustomCron ? customCron : cronExpr;
                        const preset = FREQUENCY_PRESETS.find(p => p.cron === finalCron);
                        return preset ? preset.label : <span className="font-mono">{finalCron}</span>;
                      })()}
                    </span>
                  </div>

                  {scheduleType === 'test' && (<>
                  <div className="border-t border-gray-800/50 my-2" />

                  <div className="flex justify-between">
                    <span className="text-gray-500">Runs per execution</span>
                    <span className="text-gray-200">{runs}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-gray-500">Concurrency</span>
                    <span className="text-gray-200">{concurrency}</span>
                  </div>
                  <div className="flex justify-between">
                    <span className="text-gray-500">Timeout</span>
                    <span className="text-gray-200">{timeout}s</span>
                  </div>
                  </>)}

                  {/* Non-default options */}
                  {(insecure || connectionReuse || autoStartVm || autoStopVm) && (
                    <>
                      <div className="border-t border-gray-800/50 my-2" />
                      <div className="flex flex-wrap gap-2">
                        {insecure && <span className="text-xs text-gray-500 bg-gray-800/50 px-2 py-0.5 rounded">skip TLS verify</span>}
                        {connectionReuse && <span className="text-xs text-gray-500 bg-gray-800/50 px-2 py-0.5 rounded">connection reuse</span>}
                        {autoStartVm && <span className="text-xs text-green-400/70 bg-green-500/10 px-2 py-0.5 rounded">auto-start VM</span>}
                        {autoStopVm && <span className="text-xs text-red-400/70 bg-red-500/10 px-2 py-0.5 rounded">auto-stop VM</span>}
                      </div>
                    </>
                  )}
                </div>
              </div>
            </div>
          )}

          {/* Navigation */}
          <div className="flex justify-between pt-4 border-t border-gray-800/50 mt-6">
            <button
              type="button"
              onClick={() => step > 1 ? setStep(step - 1) : onClose()}
              className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
            >
              {step === 1 ? 'Cancel' : 'Back'}
            </button>
            {step < totalSteps ? (
              <button
                type="button"
                onClick={() => {
                  setError(null);
                  if (step === 1 && !name.trim()) { setError('Name is required'); return; }
                  if (step === 1 && scheduleType === 'benchmark' && !selectedBenchmarkConfigId) { setError('Select a benchmark config template'); return; }
                  if (step === 2 && scheduleType === 'test' && selectedModes.size === 0) { setError('Select at least one mode'); return; }
                  setStep(step + 1);
                }}
                className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
              >
                Next
              </button>
            ) : (
              <button
                type="button"
                onClick={handleSubmit}
                disabled={loading}
                className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
              >
                {loading ? 'Creating...' : 'Create Schedule'}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
