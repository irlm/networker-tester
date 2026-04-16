import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { api } from '../api/client';
import type { CloudAccountSummary } from '../api/types';
import { TestbedRow } from './wizard/TestbedRow';
import {
  makeTestbed,
  updateTestbedState,
  resolveVmSize,
  PROXY_LABELS,
  type TestbedState,
} from './wizard/testbed-constants';
import { useToast } from '../hooks/useToast';

// ── Prefill: lets the Infrastructure page open the wizard with an existing
//    target's IP + already-installed proxy stacks selected. The user then
//    only ticks the new stacks they want installed (idempotent — install.sh
//    no-ops on already-installed stacks).
export interface DeployWizardPrefill {
  cloud: 'Azure' | 'AWS' | 'GCP';
  cloudAccountId: string;
  region: string;
  os: 'linux' | 'windows';
  /** IP/hostname of the existing target. */
  existingVmIp: string;
  /** Already-installed stacks — pre-selected so the user can see what's there. */
  installedProxies: string[];
}

interface DeployWizardProps {
  projectId: string;
  onClose: () => void;
  onCreated: (deploymentId: string) => void;
  /** Optional: open with these values pre-filled (used by "Add stack" on a deployed target). */
  prefill?: DeployWizardPrefill;
}

export function DeployWizard({ projectId, onClose, onCreated, prefill }: DeployWizardProps) {
  const [step, setStep] = useState<1 | 2>(1);
  const [cloudAccounts, setCloudAccounts] = useState<CloudAccountSummary[]>([]);
  const [cloudLoading, setCloudLoading] = useState(true);

  const [testbedKey, setTestbedKey] = useState(1);
  const [testbeds, setTestbeds] = useState<TestbedState[]>(() => {
    if (prefill) {
      const tb = makeTestbed(0, prefill.cloud, prefill.os, prefill.installedProxies);
      tb.cloudAccountId = prefill.cloudAccountId;
      tb.region = prefill.region;
      tb.existingVm = true;
      tb.existingVmId = prefill.existingVmIp;
      return [tb];
    }
    return [makeTestbed(0, 'Azure', 'linux')];
  });

  const [name, setName] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const addToast = useToast();

  // ── Load cloud accounts ─────────────────────────────────────────────────
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
        // Re-validate stale accounts (last_validated > 10 min ago)
        const tenMinAgo = Date.now() - 10 * 60 * 1000;
        for (const acct of list) {
          const lastVal = acct.last_validated ? new Date(acct.last_validated).getTime() : 0;
          if (lastVal < tenMinAgo) {
            api.validateCloudAccount(projectId, acct.account_id).catch(() => {});
          }
        }
      })
      .catch(() => {})
      .finally(() => setCloudLoading(false));
  }, [projectId]);

  // ── Esc to close + focus trap ───────────────────────────────────────────
  const handleKeyDown = useCallback((e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); }, [onClose]);
  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    function trapFocus(e: KeyboardEvent) {
      if (e.key !== 'Tab') return;
      const els = dialog!.querySelectorAll<HTMLElement>(
        'input:not([disabled]), button:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]):not([disabled])'
      );
      if (els.length === 0) return;
      const first = els[0];
      const last = els[els.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
    dialog.addEventListener('keydown', trapFocus);
    return () => dialog.removeEventListener('keydown', trapFocus);
  }, []);

  // ── Testbed ops ─────────────────────────────────────────────────────────
  const updateTestbed = (key: number, patch: Partial<TestbedState>) => {
    setTestbeds(prev => updateTestbedState(prev, key, patch));
  };
  const addTestbed = () => {
    const k = testbedKey;
    setTestbedKey(k + 1);
    setTestbeds(prev => [...prev, makeTestbed(k, 'Azure', 'linux')]);
  };
  const removeTestbed = (key: number) => {
    setTestbeds(prev => prev.length > 1 ? prev.filter(t => t.key !== key) : prev);
  };

  // ── Validation ──────────────────────────────────────────────────────────
  const canProceed = useMemo(() => {
    return testbeds.length > 0 && testbeds.every(tb => {
      if (tb.proxies.length === 0) return false;
      if (tb.existingVm) return tb.existingVmId.trim().length > 0;
      return tb.cloudAccountId !== '';
    });
  }, [testbeds]);

  // ── Auto-name ───────────────────────────────────────────────────────────
  const autoName = (): string => {
    return testbeds.map(tb => {
      if (tb.existingVm) return `Upgrade ${tb.existingVmId}`;
      const proxies = tb.proxies.map(p => PROXY_LABELS[p] ?? p).join('+');
      return `${tb.cloud}/${tb.region} ${tb.os} · ${proxies}`;
    }).join(' + ');
  };

  // ── Submit: TestbedState[] → install.sh deploy.json ─────────────────────
  const handleSubmit = async () => {
    setLoading(true);
    setError(null);

    const deployName = name.trim() || autoName() || 'Deployment';

    // Pick a representative cloud_account_id from the first new (non-existing)
    // testbed. install.sh accepts a single cloud_account_id at the root and
    // applies it to every endpoint that references the same provider.
    const firstNewCloud = testbeds.find(tb => !tb.existingVm);
    const cloudAccountId = firstNewCloud?.cloudAccountId;

    const config: Record<string, unknown> = {
      version: 1,
      tester: { provider: 'local' },
      ...(cloudAccountId ? { cloud_account_id: cloudAccountId } : {}),
      endpoints: testbeds.map(tb => {
        // "Use existing VM" → install over SSH/LAN, no new VM provisioned.
        // install.sh idempotently adds any http_stacks not already present.
        if (tb.existingVm) {
          return {
            provider: 'lan',
            lan: { ip: tb.existingVmId.trim(), user: 'azureuser', port: 22 },
            http_stacks: tb.proxies,
            label: `upgrade-${tb.existingVmId.trim()}`,
          };
        }
        const provider = tb.cloud.toLowerCase();
        const vmSize = resolveVmSize(tb.cloud, tb.vmSize);
        const osSuffix = tb.os === 'windows' ? 'win' : 'ubuntu';
        const suffix = Math.random().toString(36).slice(2, 6);
        const vmName = `nwk-ep-${osSuffix}-${suffix}`;
        const entry: Record<string, unknown> = { provider, http_stacks: tb.proxies };
        if (provider === 'azure') {
          entry.azure = { region: tb.region, vm_size: vmSize, os: tb.os, vm_name: vmName };
        } else if (provider === 'aws') {
          entry.aws = { region: tb.region, instance_type: vmSize, os: tb.os, instance_name: vmName };
        } else if (provider === 'gcp') {
          entry.gcp = { region: tb.region, zone: `${tb.region}-a`, machine_type: vmSize, os: tb.os, instance_name: vmName };
        }
        return entry;
      }),
      tests: { run_tests: false },
    };

    try {
      const result = await api.createDeployment(projectId, deployName, config);
      addToast('success', `Deploy ${result.deployment_id.slice(0, 8)} started`);
      onCreated(result.deployment_id);
      onClose();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to create deployment';
      setError(msg);
      addToast('error', msg);
    } finally {
      setLoading(false);
    }
  };

  const titleId = 'deploy-wizard-title';
  const totalSteps = 2;
  const upgradeMode = !!prefill;

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />

      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative w-full md:w-[640px] md:max-w-[95vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-6">
            <div className="flex items-center gap-4">
              <h3 id={titleId} className="text-lg font-bold text-gray-100">
                {upgradeMode ? 'Add Stack to Target' : 'Deploy Target'}
              </h3>
              <div className="flex gap-1">
                {Array.from({ length: totalSteps }, (_, i) => i + 1).map(s => (
                  <div key={s} className={`w-6 h-1 rounded-full ${s <= step ? 'bg-green-500' : 'bg-gray-700'}`} />
                ))}
              </div>
            </div>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">&#x2715;</button>
          </div>

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
              <p className="text-red-400 text-sm">{error}</p>
            </div>
          )}

          {step === 1 && (
            <div>
              {upgradeMode ? (
                <p className="text-sm text-gray-400 mb-3">
                  Add proxy stacks to <span className="text-cyan-400 font-mono">{prefill.existingVmIp}</span>.
                  Already-installed stacks are pre-selected; tick additional stacks to install.
                </p>
              ) : (
                <p className="text-sm text-gray-400 mb-3">Configure your target. Pick proxies, region, and size — or check "Use existing VM" to install on a host you already have.</p>
              )}

              {cloudLoading && cloudAccounts.length === 0 ? (
                <p className="text-xs text-gray-500 py-3">Loading cloud accounts...</p>
              ) : cloudAccounts.length === 0 ? (
                <div className="border border-yellow-500/30 bg-yellow-500/5 rounded p-3 mb-3 text-xs text-yellow-300">
                  No cloud accounts configured. Add one in Settings → Cloud, or check "Use existing VM" on each testbed to install on a host you already have.
                </div>
              ) : null}

              <div className="space-y-2">
                {testbeds.map((tb, idx) => (
                  <TestbedRow
                    key={tb.key}
                    testbed={tb}
                    index={idx}
                    projectId={projectId}
                    cloudAccounts={cloudAccounts}
                    onUpdate={updateTestbed}
                    onRemove={removeTestbed}
                    hideTesterOs
                  />
                ))}
              </div>

              {!upgradeMode && testbeds.length < 4 && (
                <button
                  type="button"
                  onClick={addTestbed}
                  className="mt-3 text-xs text-cyan-400 hover:text-cyan-300"
                >
                  + Add target
                </button>
              )}
            </div>
          )}

          {step === 2 && (
            <div>
              <p className="text-sm text-gray-400 mb-3">Review deployment configuration.</p>

              <div className="mb-3">
                <label className="block text-xs text-gray-400 mb-1">Deployment Name</label>
                <input
                  value={name}
                  onChange={e => setName(e.target.value)}
                  placeholder={autoName()}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>

              <div className="bg-[var(--bg-base)] border border-gray-800 rounded p-3 mb-3">
                <p className="text-xs text-gray-500 mb-2 font-medium">Targets ({testbeds.length})</p>
                {testbeds.map((tb, i) => (
                  <div key={tb.key} className="text-sm text-gray-300 py-1 flex flex-wrap items-center gap-2">
                    <span className="text-gray-500 font-mono w-4">{i + 1}</span>
                    {tb.existingVm ? (
                      <>
                        <span className="text-amber-400 text-[10px] px-1.5 py-0.5 border border-amber-500/30 rounded">upgrade</span>
                        <span className="font-mono text-cyan-400">{tb.existingVmId}</span>
                      </>
                    ) : (
                      <>
                        <span>{tb.cloud}</span>
                        <span className="text-gray-500">/</span>
                        <span className="text-gray-300">{tb.region}</span>
                        <span className="text-gray-500">·</span>
                        <span className="text-gray-400">{tb.os}</span>
                        <span className="text-gray-500">·</span>
                        <span className="text-gray-400">{tb.vmSize}</span>
                      </>
                    )}
                    <span className="text-cyan-500/70 ml-2">
                      {tb.proxies.map(p => PROXY_LABELS[p] ?? p).join(', ')}
                    </span>
                  </div>
                ))}
              </div>

              <p className="text-xs text-gray-500">
                install.sh runs idempotently — already-installed proxies are skipped, new ones added.
              </p>
            </div>
          )}

          {/* Navigation */}
          <div className="flex justify-between pt-4 border-t border-gray-800/50 mt-6">
            <div>
              {step > 1 && (
                <button type="button" onClick={() => setStep(1)} className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200">
                  Back
                </button>
              )}
            </div>
            <div className="flex gap-3">
              <button type="button" onClick={onClose} className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200">
                Cancel
              </button>
              {step === 1 ? (
                <button
                  type="button"
                  onClick={() => setStep(2)}
                  disabled={!canProceed}
                  className="bg-cyan-600 hover:bg-cyan-500 disabled:bg-gray-800 disabled:text-gray-600 disabled:cursor-not-allowed text-white px-4 py-1.5 rounded text-sm transition-colors"
                >
                  Next
                </button>
              ) : (
                <button
                  type="button"
                  onClick={handleSubmit}
                  disabled={loading}
                  className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {loading ? 'Deploying...' : upgradeMode ? 'Install Stacks' : 'Deploy Target'}
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
