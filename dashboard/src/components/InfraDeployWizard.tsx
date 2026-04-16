// ── Unified Infrastructure deploy wizard (v6 linear stepper) ─────────────
//
// One wizard, two paths: TARGET (deploys a server-under-test via install.sh)
// or RUNNER (provisions a tester VM via testersApi.createTester).
//
// Layout follows /tmp/mockups/unified-deploy/v6-linear-stepper.html: 5 steps
// with a top stepper, kind picker on step 1, then forks at step 4.

import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { api } from '../api/client';
import { testersApi, type CreateTesterBody } from '../api/testers';
import type { CloudAccountSummary } from '../api/types';
import { CloudAccountCombobox } from './wizard/CloudAccountCombobox';
import {
  REGIONS,
  INSTANCE_TYPES,
  defaultInstanceType,
  LINUX_PROXIES,
  WINDOWS_PROXIES,
  PROXY_LABELS,
} from './wizard/testbed-constants';
import { useToast } from '../hooks/useToast';

// ── Types ────────────────────────────────────────────────────────────────

type Kind = 'target' | 'runner';

export interface InfraDeployWizardProps {
  projectId: string;
  /** Pre-select kind on entry (Infrastructure page can hint based on which button was clicked). */
  initialKind?: Kind;
  /** Optional: pre-fill an existing IP for the "+ Add stack" upgrade path. Forces kind=target. */
  prefillUpgrade?: {
    cloud: 'Azure' | 'AWS' | 'GCP';
    cloudAccountId: string;
    region: string;
    os: 'linux' | 'windows';
    existingVmIp: string;
    installedProxies: string[];
  };
  onClose: () => void;
  onCreated: (kind: Kind, id: string) => void;
}

// ── Helpers ──────────────────────────────────────────────────────────────

function providerToCloud(p: string): 'Azure' | 'AWS' | 'GCP' {
  const lp = p.toLowerCase();
  if (lp === 'aws') return 'AWS';
  if (lp === 'gcp') return 'GCP';
  return 'Azure';
}

const STEPS = ['Kind', 'Cloud', 'Region & OS', 'Configure', 'Review'] as const;

// ── Component ────────────────────────────────────────────────────────────

export function InfraDeployWizard({
  projectId,
  initialKind = 'target',
  prefillUpgrade,
  onClose,
  onCreated,
}: InfraDeployWizardProps) {
  // Force target kind in upgrade mode (you can't upgrade a runner via SSH).
  const [kind, setKind] = useState<Kind>(prefillUpgrade ? 'target' : initialKind);
  const [step, setStep] = useState(prefillUpgrade ? 1 : 0);

  // Cloud account
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  const [accountId, setAccountId] = useState(prefillUpgrade?.cloudAccountId ?? '');
  const [cloud, setCloud] = useState<'Azure' | 'AWS' | 'GCP'>(prefillUpgrade?.cloud ?? 'Azure');

  // Region / instance / OS
  const [region, setRegion] = useState(prefillUpgrade?.region ?? REGIONS.Azure[0]);
  const [vmSize, setVmSize] = useState(defaultInstanceType('Azure'));
  const [os, setOs] = useState<'linux' | 'windows'>(prefillUpgrade?.os ?? 'linux');

  // Target-specific
  const [proxies, setProxies] = useState<string[]>(prefillUpgrade?.installedProxies ?? ['nginx']);
  const [useExistingVm, setUseExistingVm] = useState(!!prefillUpgrade);
  const [existingVmIp, setExistingVmIp] = useState(prefillUpgrade?.existingVmIp ?? '');

  // Runner-specific
  const [runnerName, setRunnerName] = useState('');
  const [autoShutdownEnabled, setAutoShutdownEnabled] = useState(true);
  const [autoShutdownHour, setAutoShutdownHour] = useState(23);

  // Review
  const [deployName, setDeployName] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dialogRef = useRef<HTMLDivElement>(null);
  const addToast = useToast();

  // ── Load cloud accounts ────────────────────────────────────────────────
  useEffect(() => {
    api.getCloudAccounts(projectId)
      .then(accts => {
        const list = Array.isArray(accts) ? accts : [];
        list.sort((a, b) => {
          if (a.status === 'active' && b.status !== 'active') return -1;
          if (a.status !== 'active' && b.status === 'active') return 1;
          return a.provider.localeCompare(b.provider) || a.name.localeCompare(b.name);
        });
        setCloudAccounts(list);
      })
      .catch(() => {});
  }, [projectId]);

  // ── Esc to close + focus trap ──────────────────────────────────────────
  const handleKeyDown = useCallback((e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); }, [onClose]);
  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // ── Suggest a runner name when entering step 4 (runner path) ───────────
  useEffect(() => {
    if (kind === 'runner' && step === 3 && !runnerName && region) {
      setRunnerName(`${region}-runner-01`);
    }
  }, [kind, step, region, runnerName]);

  // ── Step validation ────────────────────────────────────────────────────
  const canProceed = useMemo(() => {
    if (step === 0) return true;
    if (step === 1) return accountId !== '';
    if (step === 2) return region !== '' && vmSize !== '';
    if (step === 3) {
      if (kind === 'target') {
        if (proxies.length === 0) return false;
        if (useExistingVm && !existingVmIp.trim()) return false;
        return true;
      }
      return runnerName.trim().length > 0;
    }
    return true;
  }, [step, accountId, region, vmSize, kind, proxies, useExistingVm, existingVmIp, runnerName]);

  // ── Cloud account select handler ───────────────────────────────────────
  const onSelectAccount = (acct: CloudAccountSummary) => {
    const c = providerToCloud(acct.provider);
    const validRegion = (REGIONS[c] ?? []).includes(region)
      ? region
      : (acct.region_default && (REGIONS[c] ?? []).includes(acct.region_default)
          ? acct.region_default
          : (REGIONS[c] ?? [])[0] ?? '');
    const validSku = (INSTANCE_TYPES[c] ?? []).some(t => t.id === vmSize)
      ? vmSize
      : defaultInstanceType(c);
    setAccountId(acct.account_id);
    setCloud(c);
    setRegion(validRegion);
    setVmSize(validSku);
  };

  // ── Compute the auto-suggested deployment name for the review step ─────
  const autoDeployName = useMemo(() => {
    if (kind === 'runner') return runnerName || `${region}-runner-01`;
    if (useExistingVm) return `upgrade-${existingVmIp.trim() || 'host'}`;
    const stacks = proxies.map(p => PROXY_LABELS[p] ?? p).join('-');
    return `target-${cloud.toLowerCase()}-${region}-${stacks || 'nginx'}`;
  }, [kind, runnerName, region, useExistingVm, existingVmIp, proxies, cloud]);

  // ── Submit ─────────────────────────────────────────────────────────────
  const submit = async () => {
    setSubmitting(true);
    setError(null);
    const finalName = deployName.trim() || autoDeployName;
    try {
      if (kind === 'target') {
        const config: Record<string, unknown> = {
          version: 1,
          tester: { provider: 'local' },
          cloud_account_id: accountId,
          endpoints: [
            useExistingVm
              ? {
                  provider: 'lan',
                  lan: { ip: existingVmIp.trim(), user: 'azureuser', port: 22 },
                  http_stacks: proxies,
                  label: `upgrade-${existingVmIp.trim()}`,
                }
              : (() => {
                  const provider = cloud.toLowerCase();
                  const osSuffix = os === 'windows' ? 'win' : 'ubuntu';
                  const suffix = Math.random().toString(36).slice(2, 6);
                  const vmName = `nwk-ep-${osSuffix}-${suffix}`;
                  if (provider === 'azure') {
                    return { provider, http_stacks: proxies, azure: { region, vm_size: vmSize, os, vm_name: vmName } };
                  }
                  if (provider === 'aws') {
                    return { provider, http_stacks: proxies, aws: { region, instance_type: vmSize, os, instance_name: vmName } };
                  }
                  return { provider, http_stacks: proxies, gcp: { region, zone: `${region}-a`, machine_type: vmSize, os, instance_name: vmName } };
                })(),
          ],
          tests: { run_tests: false },
        };
        const result = await api.createDeployment(projectId, finalName, config);
        addToast('success', `Deploy ${result.deployment_id.slice(0, 8)} started`);
        onCreated('target', result.deployment_id);
        onClose();
      } else {
        const body: CreateTesterBody = {
          name: finalName,
          cloud: cloud.toLowerCase(),
          region,
          vm_size: vmSize,
          requested_os: os,
          ...(autoShutdownEnabled ? { auto_shutdown_local_hour: autoShutdownHour } : {}),
        };
        const result = await testersApi.createTester(projectId, body);
        addToast('success', `Runner "${finalName}" created`);
        onCreated('runner', result.tester_id);
        onClose();
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to deploy';
      setError(msg);
      addToast('error', msg);
    } finally {
      setSubmitting(false);
    }
  };

  // ── Render ─────────────────────────────────────────────────────────────
  const titleId = 'infra-deploy-wizard-title';
  const upgradeMode = !!prefillUpgrade;
  const accentClass = kind === 'target' ? 'text-cyan-300' : 'text-purple-300';
  const accentBorder = kind === 'target' ? 'border-cyan-500' : 'border-purple-500';
  const validProxyList = os === 'windows' ? WINDOWS_PROXIES : LINUX_PROXIES;

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative w-full md:w-[680px] md:max-w-[95vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6">
          {/* Header */}
          <div className="flex items-center justify-between mb-5">
            <h3 id={titleId} className="text-lg font-bold text-gray-100">
              {upgradeMode ? 'Add Stack to Target' : 'Deploy'}
            </h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">&#x2715;</button>
          </div>

          {/* Stepper bar — clickable to jump back */}
          <div className="flex items-center gap-0 mb-6 pb-4 border-b border-gray-800">
            {STEPS.map((label, i) => (
              <div key={label} className="flex items-center" style={{ flex: i === STEPS.length - 1 ? '0 0 auto' : '1 1 auto' }}>
                <button
                  type="button"
                  onClick={() => i <= step && setStep(i)}
                  className={`flex items-center gap-2 text-xs font-mono whitespace-nowrap ${
                    i === step ? accentClass :
                    i < step ? 'text-gray-400 hover:text-gray-200 cursor-pointer' :
                    'text-gray-600 cursor-not-allowed'
                  }`}
                >
                  <span className={`w-5 h-5 rounded-full inline-flex items-center justify-center text-[10px] border ${
                    i === step ? `${accentBorder} bg-transparent ${accentClass}` :
                    i < step ? 'border-green-500/40 bg-green-500/10 text-green-400' :
                    'border-gray-700 bg-gray-900 text-gray-600'
                  }`}>
                    {i < step ? '✓' : i + 1}
                  </span>
                  {label}
                </button>
                {i < STEPS.length - 1 && <div className="flex-1 h-px bg-gray-800 mx-3 min-w-[18px]" />}
              </div>
            ))}
          </div>

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
              <p className="text-red-400 text-xs">{error}</p>
            </div>
          )}

          {/* ── STEP 1 — Kind ────────────────────────────────────────────── */}
          {step === 0 && (
            <div>
              <h4 className="text-base font-semibold text-gray-100 mb-1">What kind of VM?</h4>
              <p className="text-xs text-gray-500 mb-4">Targets are servers you measure against. Runners are the agents that send the probes.</p>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                {([
                  { id: 'target' as const, color: 'cyan', title: '▢ Server-under-test', desc: 'Receives probe traffic. Hosts one or more reverse-proxy stacks (nginx, IIS, Caddy, Traefik, HAProxy, Apache). Can be a fresh VM or an existing host installed over SSH.' },
                  { id: 'runner' as const, color: 'purple', title: '↗ Load-generator agent', desc: 'Runs the networker-agent binary. Connects to this dashboard over WebSocket and executes probe jobs against targets. Auto-shutdown supported.' },
                ]).map(card => {
                  const selected = kind === card.id;
                  return (
                    <button
                      key={card.id}
                      type="button"
                      onClick={() => setKind(card.id)}
                      className={`text-left p-4 border transition-colors ${
                        selected
                          ? card.color === 'cyan'
                            ? 'border-cyan-500/50 bg-cyan-500/10'
                            : 'border-purple-500/50 bg-purple-500/10'
                          : 'border-gray-800 hover:border-gray-600'
                      }`}
                    >
                      <span className={`inline-block font-mono text-[9px] tracking-wider px-1.5 py-0.5 border mb-2 ${
                        card.color === 'cyan'
                          ? 'bg-cyan-500/15 border-cyan-500/40 text-cyan-300'
                          : 'bg-purple-500/15 border-purple-500/40 text-purple-300'
                      }`}>{card.id.toUpperCase()}</span>
                      <h5 className="text-sm font-medium text-gray-100 mb-1">{card.title}</h5>
                      <p className="text-xs text-gray-500 font-mono leading-relaxed">{card.desc}</p>
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {/* ── STEP 2 — Cloud ───────────────────────────────────────────── */}
          {step === 1 && (
            <div>
              <h4 className="text-base font-semibold text-gray-100 mb-1">Which cloud account?</h4>
              <p className="text-xs text-gray-500 mb-4">Pick from validated accounts. The combobox supports type-ahead — try typing "azure" or "prod".</p>
              <CloudAccountCombobox
                projectId={projectId}
                cloudAccounts={cloudAccounts}
                selectedAccountId={accountId}
                onSelect={onSelectAccount}
              />
            </div>
          )}

          {/* ── STEP 3 — Region & OS ─────────────────────────────────────── */}
          {step === 2 && (
            <div>
              <h4 className="text-base font-semibold text-gray-100 mb-1">Region, instance type, and OS</h4>
              <p className="text-xs text-gray-500 mb-4">Cloud-native instance type names (no Small/Medium/Large abstraction).</p>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs text-gray-500 mb-1">Region</label>
                  <select
                    value={region}
                    onChange={e => setRegion(e.target.value)}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                  >
                    {(REGIONS[cloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
                  </select>
                </div>
                <div>
                  <label className="block text-xs text-gray-500 mb-1">Instance type</label>
                  <select
                    value={vmSize}
                    onChange={e => setVmSize(e.target.value)}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500"
                  >
                    {(INSTANCE_TYPES[cloud] ?? []).map(t => (
                      <option key={t.id} value={t.id}>{t.id} · {t.hint}</option>
                    ))}
                  </select>
                </div>
                <div className="md:col-span-2">
                  <label className="block text-xs text-gray-500 mb-1">Operating system</label>
                  <div className="flex">
                    {(['linux', 'windows'] as const).map(o => (
                      <button
                        key={o}
                        type="button"
                        onClick={() => setOs(o)}
                        className={`px-3 py-1.5 text-xs font-mono border transition-colors ${
                          os === o
                            ? o === 'linux'
                              ? 'bg-green-500/10 border-green-500/40 text-green-300 z-10'
                              : 'bg-blue-500/10 border-blue-500/40 text-blue-300 z-10'
                            : 'border-gray-700 text-gray-500 hover:text-gray-300'
                        } ${o === 'linux' ? '' : '-ml-px'}`}
                      >
                        {o === 'linux' ? 'Linux (Ubuntu)' : 'Windows Server'}
                      </button>
                    ))}
                  </div>
                  {kind === 'target' && (
                    <p className="text-[11px] text-gray-600 mt-2 font-mono">
                      ⓘ Linux unlocks all 5 proxy stacks; Windows adds IIS but excludes native Caddy / HAProxy packages.
                    </p>
                  )}
                  {kind === 'runner' && (
                    <p className="text-[11px] text-gray-600 mt-2 font-mono">
                      ⓘ Runners are typically Linux. Windows runners are supported but require additional setup.
                    </p>
                  )}
                </div>
              </div>
            </div>
          )}

          {/* ── STEP 4 — Configure (kind-specific) ───────────────────────── */}
          {step === 3 && kind === 'target' && (
            <div>
              <h4 className="text-base font-semibold text-gray-100 mb-1">Configure target</h4>
              <p className="text-xs text-gray-500 mb-4">Pick which proxy stacks to install. install.sh runs idempotently — already-installed stacks are skipped.</p>

              <div className="bg-cyan-500/5 border-l-2 border-cyan-500 px-3 py-1.5 mb-4 text-[10px] font-mono text-cyan-400 tracking-wider uppercase">
                ▢ Target-only fields
              </div>

              <label className="block text-xs text-gray-500 mb-2">Reverse proxies</label>
              <div className="flex flex-wrap gap-2 mb-4">
                {validProxyList.map(p => {
                  const active = proxies.includes(p);
                  return (
                    <button
                      key={p}
                      type="button"
                      onClick={() => setProxies(prev => active ? prev.filter(x => x !== p) : [...prev, p])}
                      className={`px-3 py-1 text-xs border transition-colors ${
                        active
                          ? 'bg-cyan-900/40 border-cyan-700 text-cyan-300'
                          : 'border-gray-700 text-gray-400 hover:border-gray-600'
                      }`}
                    >
                      {PROXY_LABELS[p] ?? p}
                    </button>
                  );
                })}
              </div>
              {proxies.length === 0 && (
                <p className="text-xs text-yellow-500 mb-3">At least one proxy is required</p>
              )}

              <label className="flex items-center gap-2 text-xs text-gray-300 cursor-pointer mb-2">
                <input
                  type="checkbox"
                  checked={useExistingVm}
                  onChange={e => setUseExistingVm(e.target.checked)}
                  className="accent-cyan-400"
                  disabled={upgradeMode}
                />
                Use existing VM (install over SSH/LAN — no new VM provisioned)
              </label>
              {useExistingVm && (
                <input
                  type="text"
                  value={existingVmIp}
                  onChange={e => setExistingVmIp(e.target.value)}
                  placeholder="VM IP or hostname"
                  className="bg-[var(--bg-base)] border border-gray-700 px-3 py-1.5 text-xs font-mono text-gray-300 w-72 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                />
              )}
            </div>
          )}

          {step === 3 && kind === 'runner' && (
            <div>
              <h4 className="text-base font-semibold text-gray-100 mb-1">Configure runner</h4>
              <p className="text-xs text-gray-500 mb-4">Name the runner so you can find it in the regional list. Auto-shutdown saves cost when the runner is idle past business hours.</p>

              <div className="bg-purple-500/5 border-l-2 border-purple-500 px-3 py-1.5 mb-4 text-[10px] font-mono text-purple-400 tracking-wider uppercase">
                ↗ Runner-only fields
              </div>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                <div>
                  <label className="block text-xs text-gray-500 mb-1">Name</label>
                  <input
                    type="text"
                    value={runnerName}
                    onChange={e => setRunnerName(e.target.value)}
                    placeholder={`${region}-runner-01`}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
                  />
                  <p className="text-[11px] text-gray-600 mt-1 font-mono">
                    ⓘ Suggested format: {`{region}-runner-{nn}`}. Must be unique within this project.
                  </p>
                </div>
                <div>
                  <label className="block text-xs text-gray-500 mb-1">Auto-shutdown hour (local)</label>
                  <input
                    type="number"
                    min={0}
                    max={23}
                    value={autoShutdownHour}
                    onChange={e => setAutoShutdownHour(Number(e.target.value))}
                    disabled={!autoShutdownEnabled}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500 disabled:opacity-40"
                  />
                  <p className="text-[11px] text-gray-600 mt-1 font-mono">
                    ⓘ Runner deallocates if idle past this hour. Default = 23:00.
                  </p>
                </div>
                <div className="md:col-span-2">
                  <label className="flex items-center gap-2 text-xs text-gray-300 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={autoShutdownEnabled}
                      onChange={e => setAutoShutdownEnabled(e.target.checked)}
                      className="accent-purple-400"
                    />
                    Enable auto-shutdown when idle (recommended)
                  </label>
                </div>
              </div>
            </div>
          )}

          {/* ── STEP 5 — Review ──────────────────────────────────────────── */}
          {step === 4 && (
            <div>
              <h4 className="text-base font-semibold text-gray-100 mb-1">Review and deploy</h4>
              <p className="text-xs text-gray-500 mb-4">Click any row in the stepper to jump back. Deploying spawns the install job and streams logs.</p>

              <div className="space-y-1 mb-4">
                {[
                  { k: 'Kind', v: kind === 'target'
                    ? <><span className="font-mono text-[9px] px-1.5 py-0.5 bg-cyan-500/15 border border-cyan-500/40 text-cyan-300 mr-2">▢ TARGET</span>Server-under-test</>
                    : <><span className="font-mono text-[9px] px-1.5 py-0.5 bg-purple-500/15 border border-purple-500/40 text-purple-300 mr-2">↗ RUNNER</span>Load-generator agent</>
                  },
                  { k: 'Cloud account', v: cloudAccounts.find(a => a.account_id === accountId)?.name ?? '—' },
                  { k: 'Region', v: region },
                  {
                    k: 'Instance type',
                    v: <span>{vmSize} <span className="text-gray-600">· {(INSTANCE_TYPES[cloud] ?? []).find(t => t.id === vmSize)?.hint ?? ''}</span></span>,
                  },
                  { k: 'Operating system', v: os === 'linux' ? 'Linux (Ubuntu)' : 'Windows Server' },
                  ...(kind === 'target' ? [
                    { k: 'Reverse proxies', v: <span className="text-cyan-400">{proxies.map(p => PROXY_LABELS[p] ?? p).join(', ')}</span> },
                    { k: 'Existing VM', v: useExistingVm ? <span className="font-mono text-cyan-400">{existingVmIp}</span> : <span className="text-gray-500">no — provisioning new</span> },
                  ] : [
                    { k: 'Name', v: <span className="font-mono">{runnerName}</span> },
                    { k: 'Auto-shutdown', v: autoShutdownEnabled ? <span className="font-mono">{String(autoShutdownHour).padStart(2, '0')}:00 local</span> : <span className="text-gray-500">disabled</span> },
                  ]),
                ].map((row, idx) => (
                  <div key={idx} className="flex items-baseline gap-3 px-3 py-2 border border-gray-800 bg-[var(--bg-raised)] font-mono text-xs">
                    <span className="text-gray-500 w-32 flex-shrink-0 text-[10px] tracking-wider uppercase">{row.k}</span>
                    <span className="text-gray-200 flex-1">{row.v}</span>
                  </div>
                ))}
              </div>

              <label className="block text-xs text-gray-500 mb-1">Deployment name</label>
              <input
                type="text"
                value={deployName}
                onChange={e => setDeployName(e.target.value)}
                placeholder={autoDeployName}
                className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
              />
            </div>
          )}

          {/* Navigation */}
          <div className="flex justify-between pt-4 border-t border-gray-800 mt-6">
            <button
              type="button"
              onClick={() => step > 0 ? setStep(s => s - 1) : onClose()}
              className="text-xs text-gray-400 hover:text-gray-200 px-2"
            >
              {step === 0 ? 'Cancel' : '← Back'}
            </button>
            {step < STEPS.length - 1 ? (
              <button
                type="button"
                disabled={!canProceed}
                onClick={() => setStep(s => s + 1)}
                className={`px-5 py-2 text-xs font-medium transition-colors ${
                  kind === 'target' ? 'bg-cyan-600 hover:bg-cyan-500' : 'bg-purple-600 hover:bg-purple-500'
                } disabled:bg-gray-800 disabled:text-gray-600 disabled:cursor-not-allowed text-white`}
              >
                Next: {STEPS[step + 1]} →
              </button>
            ) : (
              <button
                type="button"
                disabled={submitting}
                onClick={submit}
                className={`px-5 py-2 text-xs font-medium transition-colors ${
                  kind === 'target' ? 'bg-cyan-600 hover:bg-cyan-500' : 'bg-purple-600 hover:bg-purple-500'
                } disabled:opacity-50 disabled:cursor-not-allowed text-white`}
              >
                {submitting ? 'Deploying…' : kind === 'target' ? 'Deploy target' : 'Create runner'}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
