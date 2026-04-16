import { useState, useEffect, useMemo, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { testersApi, type TesterRow } from '../api/testers';
import type { EndpointRef, Workload, ModeGroup, Deployment, TestConfigCreate, CloudAccountSummary } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { WizardStepper } from '../components/wizard/WizardStepper';
import { WorkloadPanel } from '../components/wizard/WorkloadPanel';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { DEPLOY_REGIONS, HTTP_STACKS } from '../components/wizard/testbed-constants';

// ── Constants ──────────────────────────────────────────────────────────

const STEPS = ['Target', 'Workload', 'Review'];

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

// ── Component ──────────────────────────────────────────────────────────

export function NetworkTestPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Network Test');

  const [step, setStep] = useState(0);

  // Step 0: Target
  const [targetType, setTargetType] = useState<'network' | 'proxy'>('network');
  const [host, setHost] = useState('');
  const [port, setPort] = useState('');
  // Proxy target
  const [proxyEndpointId, setProxyEndpointId] = useState('');
  const [proxySubType, setProxySubType] = useState<'existing' | 'create'>('existing');
  const [newTargetAccountId, setNewTargetAccountId] = useState('');
  const [newTargetRegion, setNewTargetRegion] = useState('');
  const [newTargetOs, setNewTargetOs] = useState<'linux' | 'windows'>('linux');
  const [newTargetHttpStack, setNewTargetHttpStack] = useState('nginx');
  const [newTargetEphemeral, setNewTargetEphemeral] = useState(true);
  // Runner
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Step 1: Workload
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['http2']));
  const [runs, setRuns] = useState(10);
  const [concurrency, setConcurrency] = useState(1);
  const [timeoutMs, setTimeoutMs] = useState(5000);
  const [selectedPayloads, setSelectedPayloads] = useState<Set<string>>(new Set());
  const [insecure, setInsecure] = useState(false);
  const [connectionReuse, setConnectionReuse] = useState(true);
  const [captureMode, setCaptureMode] = useState<'none' | 'tester' | 'endpoint' | 'both'>('none');

  // Step 2: Review
  const [configName, setConfigName] = useState('');
  const [addSchedule, setAddSchedule] = useState(false);
  const [cronExpr, setCronExpr] = useState('0 0 * * * *');
  const [submitting, setSubmitting] = useState(false);

  // Shared data
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [deploymentsLoading, setDeploymentsLoading] = useState(false);
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  const [cloudAccountsLoading, setCloudAccountsLoading] = useState(false);
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [testersLoading, setTestersLoading] = useState(false);

  // ── Data loading ────────────────────────────────────────────────────

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
        if (list.length > 0) setNewTargetAccountId(list[0].account_id);
      })
      .catch(() => setCloudAccounts([]))
      .finally(() => setCloudAccountsLoading(false));
  }, [projectId]);

  // ── Navigation ──────────────────────────────────────────────────────

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
    const idle = online.filter(t => t.allocation === 'idle');
    return { online: online.length, idle: idle.length };
  }, [testers]);

  const canNext = useMemo(() => {
    if (step === 0) {
      if (targetType === 'network') return host.trim().length > 0;
      if (proxySubType === 'existing') return proxyEndpointId.trim().length > 0;
      return newTargetAccountId !== '' && newTargetRegion !== '';
    }
    if (step === 1) return selectedModes.size > 0;
    if (step === 2) return configName.trim().length > 0;
    return true;
  }, [step, targetType, host, proxyEndpointId, proxySubType, newTargetAccountId, newTargetRegion, selectedModes.size, configName]);

  const goNext = useCallback(() => {
    if (!canNext || step >= 2) return;
    setStep(step + 1);
  }, [canNext, step]);

  const goBack = useCallback(() => {
    if (step > 0) setStep(step - 1);
  }, [step]);

  // ── Submit ──────────────────────────────────────────────────────────

  const buildEndpoint = (): EndpointRef => {
    if (targetType === 'network') {
      return { kind: 'network', host, ...(port ? { port: Number(port) } : {}) };
    }
    return { kind: 'proxy', proxy_endpoint_id: proxyEndpointId };
  };

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

      const config: TestConfigCreate = {
        name: configName,
        endpoint: buildEndpoint(),
        workload,
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
      <Breadcrumb items={[{ label: 'Network Tests', to: `/projects/${projectId}/runs` }, { label: 'New Test' }]} />

      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">New Network Test</h2>
        <p className="text-xs text-gray-500 mt-1">
          Test network performance against any host or deployed target.
        </p>
      </div>

      <WizardStepper steps={STEPS} currentStep={step} onStepClick={setStep} />

      {/* ── Step 0: Target ── */}
      {step === 0 && (
        <div>
          {/* Target type selector */}
          <div className="mb-6">
            <label className="text-xs text-gray-500 mb-2 block">Target Type</label>
            <div className="flex">
              {([
                { kind: 'network' as const, label: 'Host' },
                { kind: 'proxy' as const, label: 'Deployed Target' },
              ]).map(({ kind, label }) => (
                <button
                  key={kind}
                  onClick={() => setTargetType(kind)}
                  className={`px-3 py-1.5 text-xs font-mono border transition-colors ${
                    targetType === kind
                      ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300 z-10'
                      : 'border-gray-700 text-gray-500 hover:text-gray-300'
                  } ${kind === 'network' ? '' : '-ml-px'}`}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>

          {/* Network: host + port */}
          {targetType === 'network' && (
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
          {targetType === 'proxy' && (
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
                    {sub === 'existing' ? 'Use existing target' : 'Deploy new target'}
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
                    Remove target after test completes
                  </label>
                </div>
              )}
            </div>
          )}

          {/* Runner selection */}
          <div className="mt-6 border border-gray-800 p-4">
            <div className="flex items-center justify-between mb-3">
              <h4 className="text-xs font-semibold text-gray-300 tracking-wider">Runner</h4>
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
              <p className="text-xs text-gray-500">A runner will be auto-assigned.</p>
            )}
            {runnerMode === 'specific' && (
              <div className="space-y-1">
                {testersLoading && <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading runners...</p>}
                {!testersLoading && testers.length === 0 && <p className="text-xs text-gray-500">No runners available.</p>}
                {!testersLoading && testers.length > 0 && testers.map(row => {
                  const isOnline = row.power_state === 'running';
                  const checked = selectedTesterId === row.tester_id;
                  return (
                    <label
                      key={row.tester_id}
                      className={`block border p-2.5 transition-colors ${
                        !isOnline ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'
                      } ${checked ? 'border-cyan-500/50 bg-cyan-500/5' : 'border-gray-800 hover:border-gray-600'}`}
                    >
                      <div className="flex items-center gap-3">
                        <input type="radio" name="runner" value={row.tester_id} checked={checked} disabled={!isOnline} onChange={() => setSelectedTesterId(row.tester_id)} className="accent-cyan-400" />
                        <span className="text-sm font-medium text-gray-100 flex-1">{row.name}</span>
                        <span className="text-[10px] font-mono text-gray-500">{row.cloud} / {row.region}</span>
                        <span className={`text-[10px] font-mono px-1.5 py-0.5 border rounded ${testerStatusClass(row)}`}>{testerStatusLabel(row)}</span>
                      </div>
                    </label>
                  );
                })}
              </div>
            )}
          </div>
        </div>
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

      {/* ── Step 2: Review ── */}
      {step === 2 && (
        <div>
          <h3 className="text-sm font-semibold text-gray-200 mb-4">Review & Launch</h3>

          <label className="text-xs text-gray-500 block mb-4">
            Test name
            <input
              type="text"
              value={configName}
              onChange={e => setConfigName(e.target.value)}
              placeholder="e.g. cloudflare-http2-daily"
              className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
            />
          </label>

          {/* Target info */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Target</div>
            <div className="text-xs font-mono text-gray-400">
              {targetType === 'network' && `${host}${port ? `:${port}` : ''}`}
              {targetType === 'proxy' && proxySubType === 'existing' && selectedDeployment && `${selectedDeployment.name}`}
              {targetType === 'proxy' && proxySubType === 'create' && `New (${newTargetOs}, ${newTargetHttpStack})`}
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

          {/* Workload */}
          <div className="mb-4">
            <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Workload</div>
            <div className="text-xs font-mono text-gray-400">
              {runs} runs x {concurrency} concurrency / {timeoutMs}ms timeout / {[...selectedModes].join(' ')}
            </div>
          </div>

          {insecure && <p className="text-xs text-yellow-400 mb-2">Insecure mode (TLS verification disabled)</p>}
          {targetType === 'proxy' && proxySubType === 'create' && (
            <div className="bg-blue-500/10 border border-blue-500/30 p-2 text-xs text-blue-300 mb-4">
              Target will be deployed first (~2 min).
              {newTargetEphemeral && ' Auto-teardown after run.'}
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
              ) : 'Launch Now'}
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
        {step < 2 && (
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
