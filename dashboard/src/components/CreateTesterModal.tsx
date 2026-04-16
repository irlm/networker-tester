import { useCallback, useEffect, useRef, useState } from 'react';
import { testersApi, type TesterRow } from '../api/testers';
import { api } from '../api/client';

interface CreateTesterModalProps {
  projectId: string;
  defaultCloud?: string;
  defaultRegion?: string;
  defaultName?: string;
  defaultVmSize?: string;
  defaultAutoShutdownEnabled?: boolean;
  defaultAutoShutdownHour?: number;
  /**
   * Pre-fill the Operating System selector (e.g. `ubuntu-24.04`,
   * `windows-11`). Must be a value present in `OS_OPTIONS[cloud]` or
   * the select will silently render the first entry.
   */
  defaultOs?: string;
  /**
   * Pre-fill the variant selector (`server` or `desktop`). Ignored if
   * not supported by the chosen `defaultOs`.
   */
  defaultVariant?: string;
  onCreated: (testerId: string) => void;
  onClose: () => void;
}

const VM_SIZE_PRESETS: Record<string, { value: string; label: string }[]> = {
  azure: [
    { value: 'Standard_B1s', label: 'Standard_B1s (1 vCPU, 1 GB) — cheapest' },
    { value: 'Standard_B2s', label: 'Standard_B2s (2 vCPU, 4 GB) — recommended' },
    { value: 'Standard_D2s_v3', label: 'Standard_D2s_v3 (2 vCPU, 8 GB)' },
    { value: 'Standard_D4s_v3', label: 'Standard_D4s_v3 (4 vCPU, 16 GB)' },
    { value: 'Standard_D8s_v3', label: 'Standard_D8s_v3 (8 vCPU, 32 GB)' },
  ],
  aws: [
    { value: 't3.micro', label: 't3.micro (2 vCPU, 1 GB) — cheapest' },
    { value: 't3.small', label: 't3.small (2 vCPU, 2 GB) — recommended' },
    { value: 't3.medium', label: 't3.medium (2 vCPU, 4 GB)' },
    { value: 't3.large', label: 't3.large (2 vCPU, 8 GB)' },
    { value: 'm5.large', label: 'm5.large (2 vCPU, 8 GB)' },
    { value: 'm5.xlarge', label: 'm5.xlarge (4 vCPU, 16 GB)' },
  ],
  gcp: [
    { value: 'e2-micro', label: 'e2-micro (2 vCPU, 1 GB) — cheapest' },
    { value: 'e2-small', label: 'e2-small (2 vCPU, 2 GB) — recommended' },
    { value: 'e2-medium', label: 'e2-medium (2 vCPU, 4 GB)' },
    { value: 'e2-standard-2', label: 'e2-standard-2 (2 vCPU, 8 GB)' },
    { value: 'e2-standard-4', label: 'e2-standard-4 (4 vCPU, 16 GB)' },
  ],
};

// Recommended default: 2 vCPU / 2-4 GB RAM is plenty for HTTP/TLS/DNS probes
const DEFAULT_VM_SIZE: Record<string, string> = {
  azure: 'Standard_B2s',
  aws: 't3.small',
  gcp: 'e2-small',
};

// OS options per cloud per variant. "--" means not supported.
const OS_OPTIONS: Record<string, { value: string; label: string; variants: string[] }[]> = {
  azure: [
    { value: 'ubuntu-24.04', label: 'Ubuntu 24.04 LTS', variants: ['server', 'desktop'] },
    { value: 'ubuntu-22.04', label: 'Ubuntu 22.04 LTS', variants: ['server'] },
    { value: 'debian-12',    label: 'Debian 12',         variants: ['server'] },
    { value: 'windows-2022', label: 'Windows Server 2022', variants: ['server'] },
    { value: 'windows-11',   label: 'Windows 11',        variants: ['desktop'] },
  ],
  aws: [
    { value: 'ubuntu-24.04', label: 'Ubuntu 24.04 LTS', variants: ['server'] },
    { value: 'ubuntu-22.04', label: 'Ubuntu 22.04 LTS', variants: ['server'] },
    { value: 'debian-12',    label: 'Debian 12',         variants: ['server'] },
    { value: 'windows-2022', label: 'Windows Server 2022', variants: ['server'] },
  ],
  gcp: [
    { value: 'ubuntu-24.04', label: 'Ubuntu 24.04 LTS', variants: ['server'] },
    { value: 'ubuntu-22.04', label: 'Ubuntu 22.04 LTS', variants: ['server'] },
    { value: 'debian-12',    label: 'Debian 12',         variants: ['server'] },
    { value: 'windows-2022', label: 'Windows Server 2022', variants: ['server'] },
  ],
};

const REGIONS_BY_CLOUD: Record<string, string[]> = {
  azure: [
    'eastus', 'eastus2', 'westus2', 'westus3', 'centralus',
    'northeurope', 'westeurope', 'uksouth', 'francecentral', 'germanywestcentral',
    'japaneast', 'koreacentral', 'southeastasia', 'australiaeast',
    'brazilsouth', 'canadacentral',
  ],
  aws: [
    'us-east-1', 'us-east-2', 'us-west-1', 'us-west-2',
    'eu-west-1', 'eu-west-2', 'eu-central-1',
    'ap-northeast-1', 'ap-southeast-1', 'ap-southeast-2',
    'sa-east-1', 'ca-central-1',
  ],
  gcp: [
    'us-central1', 'us-east1', 'us-east4', 'us-west1', 'us-west2', 'us-west4',
    'europe-west1', 'europe-west2', 'europe-west3', 'europe-west4',
    'asia-east1', 'asia-northeast1', 'asia-southeast1',
    'australia-southeast1',
  ],
};

const HOURS = Array.from({ length: 24 }, (_, h) => h);

type Stage = 'form' | 'creating' | 'error';

export function CreateTesterModal({
  projectId,
  defaultCloud = 'azure',
  defaultRegion = '',
  defaultName = '',
  defaultVmSize = 'Standard_D2s_v3',
  defaultAutoShutdownEnabled = true,
  defaultAutoShutdownHour = 23,
  defaultOs,
  defaultVariant,
  onCreated,
  onClose,
}: CreateTesterModalProps) {
  const [cloud, setCloud] = useState(defaultCloud);
  const [region, setRegion] = useState(defaultRegion);
  const [name, setName] = useState(defaultName);
  const [vmSize, setVmSize] = useState(defaultVmSize);
  const [requestedOs, setRequestedOs] = useState(defaultOs ?? 'ubuntu-24.04');
  const [requestedVariant, setRequestedVariant] = useState(
    defaultVariant ?? 'server',
  );
  const [autoShutdownEnabled, setAutoShutdownEnabled] = useState(
    defaultAutoShutdownEnabled,
  );
  const [autoShutdownHour, setAutoShutdownHour] = useState(
    defaultAutoShutdownHour,
  );
  const [autoProbeEnabled, setAutoProbeEnabled] = useState(false);

  const [availableClouds, setAvailableClouds] = useState<string[]>([]);
  const [existingNames, setExistingNames] = useState<Set<string>>(new Set());
  const [regions, setRegions] = useState<string[]>([]);
  const [stage, setStage] = useState<Stage>('form');
  const [error, setError] = useState<string | null>(null);
  const [createdTester, setCreatedTester] = useState<TesterRow | null>(null);

  const dialogRef = useRef<HTMLDivElement>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    firstInputRef.current?.focus();
  }, []);

  // Load available clouds from project's cloud connections AND cloud accounts
  // Also load existing tester names for unique-name suggestion
  useEffect(() => {
    let cancelled = false;
    Promise.all([
      api.getCloudConnections(projectId).catch(() => []),
      api.getCloudAccounts(projectId).catch(() => []),
      testersApi.listTesters(projectId).catch(() => []),
    ]).then(([conns, accts, testers]) => {
      if (cancelled) return;
      const connsArr = Array.isArray(conns) ? conns : [];
      const acctsArr = Array.isArray(accts) ? accts : [];
      const testersArr = Array.isArray(testers) ? testers : [];
      const fromConns = connsArr
        .filter((c: { status: string }) => c.status === 'active')
        .map((c: { provider: string }) => c.provider);
      const fromAccts = acctsArr
        .filter((a: { status: string }) => a.status === 'active')
        .map((a: { provider: string }) => a.provider);
      const providers = [...new Set([...fromConns, ...fromAccts])] as string[];
      setAvailableClouds(providers.length > 0 ? providers : ['azure']);
      setExistingNames(new Set(testersArr.map((t: { name: string }) => t.name)));
      // If current cloud not in available list, switch to first available
      if (providers.length > 0 && !providers.includes(cloud)) {
        setCloud(providers[0]);
        setVmSize(DEFAULT_VM_SIZE[providers[0]] || providers[0]);
      }
    });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  // Suggest a unique tester name based on cloud + region
  useEffect(() => {
    if (name) return; // don't override user input
    if (existingNames.size === 0 && !availableClouds.length) return;
    const base = `${cloud}-${region || 'tester'}`;
    if (!existingNames.has(base)) {
      setName(base);
      return;
    }
    // Find the next available -NN suffix
    for (let i = 1; i <= 99; i++) {
      const candidate = `${base}-${String(i).padStart(2, '0')}`;
      if (!existingNames.has(candidate)) {
        setName(candidate);
        return;
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cloud, region, existingNames, availableClouds]);

  // Load regions when cloud changes (per-cloud static list)
  useEffect(() => {
    const list = REGIONS_BY_CLOUD[cloud] || [];
    setRegions(list);
    // If current region is not valid for this cloud, reset to first available
    if (list.length > 0 && !list.includes(region)) {
      setRegion(defaultRegion && list.includes(defaultRegion) ? defaultRegion : list[0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cloud]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape' && stage !== 'creating') onClose();
    },
    [onClose, stage],
  );

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // Poll every 2s while the tester is provisioning/starting so we can show
  // status_message updates. Simpler than opening a WS for this one-shot flow.
  useEffect(() => {
    if (stage !== 'creating' || !createdTester) return;
    let cancelled = false;

    const poll = async () => {
      try {
        const row = await testersApi.getTester(projectId, createdTester.tester_id);
        if (cancelled) return;
        setCreatedTester(row);
        if (row.power_state === 'running' && row.allocation === 'idle') {
          onCreated(row.tester_id);
          return;
        }
        if (row.power_state === 'error') {
          setError(row.status_message ?? 'Runner failed to provision');
          setStage('error');
          return;
        }
        pollTimer.current = setTimeout(poll, 2000);
      } catch (e) {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : 'Status poll failed');
        setStage('error');
      }
    };

    pollTimer.current = setTimeout(poll, 2000);
    return () => {
      cancelled = true;
      if (pollTimer.current) clearTimeout(pollTimer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stage, createdTester?.tester_id]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!cloud || !region || !name || !vmSize) {
      setError('All fields are required');
      return;
    }
    setError(null);
    setStage('creating');
    try {
      const row = await testersApi.createTester(projectId, {
        cloud,
        region,
        name,
        vm_size: vmSize,
        auto_shutdown_local_hour: autoShutdownEnabled
          ? autoShutdownHour
          : undefined,
        auto_probe_enabled: autoProbeEnabled,
        requested_os: requestedOs,
        requested_variant: requestedVariant,
      });
      setCreatedTester(row);
      // If backend replied with a terminal state already (unlikely but possible),
      // short-circuit the poll.
      if (row.power_state === 'running' && row.allocation === 'idle') {
        onCreated(row.tester_id);
      } else if (row.power_state === 'error') {
        setError(row.status_message ?? 'Runner failed to provision');
        setStage('error');
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create runner');
      setStage('error');
    }
  };

  const titleId = 'create-tester-modal-title';

  return (
    <div className="fixed inset-0 z-50 flex justify-end" data-testid="create-tester-modal">
      <div
        className="absolute inset-0 bg-black/40 slide-over-backdrop"
        onClick={stage === 'creating' ? undefined : onClose}
        aria-hidden="true"
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-6">
            <h3 id={titleId} className="text-lg font-bold text-gray-100">
              Create Runner
            </h3>
            <button
              type="button"
              onClick={onClose}
              disabled={stage === 'creating'}
              className="text-gray-500 hover:text-gray-300 text-sm disabled:opacity-50"
              aria-label="Close"
            >
              &#x2715;
            </button>
          </div>

          {error && (
            <div
              role="alert"
              className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4"
            >
              <p className="text-red-400 text-sm">{error}</p>
            </div>
          )}

          {stage === 'creating' && createdTester ? (
            <div className="space-y-4" data-testid="creating-state">
              <p className="text-sm text-gray-300">
                Provisioning <span className="font-mono">{createdTester.name}</span> in{' '}
                <span className="font-mono">{createdTester.region}</span>…
              </p>
              <div className="bg-gray-900/40 border border-gray-800 rounded p-3 font-mono text-xs text-cyan-400">
                <div>power_state: {createdTester.power_state}</div>
                <div>allocation: {createdTester.allocation}</div>
                {createdTester.status_message && (
                  <div className="text-gray-400 mt-1">
                    {createdTester.status_message}
                  </div>
                )}
              </div>
              <p className="text-xs text-gray-500">
                This usually takes 2-4 minutes. You can close this dialog; the
                runner will continue provisioning in the background.
              </p>
              <div className="flex justify-end">
                <button
                  type="button"
                  onClick={onClose}
                  className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
                >
                  Close
                </button>
              </div>
            </div>
          ) : (
            <form onSubmit={handleSubmit} className="space-y-4">
              {/* Cloud */}
              <div>
                <label htmlFor="tester-cloud" className="block text-xs text-gray-400 mb-1">
                  Cloud
                </label>
                <select
                  id="tester-cloud"
                  value={cloud}
                  onChange={(e) => {
                    const newCloud = e.target.value;
                    setCloud(newCloud);
                    setVmSize(DEFAULT_VM_SIZE[newCloud] || '');
                    setRegion('');
                  }}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                >
                  {availableClouds.map((c) => (
                    <option key={c} value={c}>
                      {c === 'azure' ? 'Azure' : c === 'aws' ? 'AWS' : c === 'gcp' ? 'GCP' : c}
                    </option>
                  ))}
                </select>
              </div>

              {/* Region */}
              <div>
                <label htmlFor="tester-region" className="block text-xs text-gray-400 mb-1">
                  Region
                </label>
                <select
                  id="tester-region"
                  value={region}
                  onChange={(e) => setRegion(e.target.value)}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                >
                  {regions.length === 0 && <option value="">(no regions)</option>}
                  {regions.map((r) => (
                    <option key={r} value={r}>
                      {r}
                    </option>
                  ))}
                </select>
              </div>

              {/* Name */}
              <div>
                <label htmlFor="tester-name" className="block text-xs text-gray-400 mb-1">
                  Name
                </label>
                <input
                  ref={firstInputRef}
                  id="tester-name"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="eastus-1"
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
              </div>

              {/* OS */}
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label htmlFor="tester-os" className="block text-xs text-gray-400 mb-1">
                    Operating System
                  </label>
                  <select
                    id="tester-os"
                    value={requestedOs}
                    onChange={(e) => {
                      const newOs = e.target.value;
                      setRequestedOs(newOs);
                      const opts = OS_OPTIONS[cloud] || [];
                      const osDef = opts.find((o) => o.value === newOs);
                      if (osDef && !osDef.variants.includes(requestedVariant)) {
                        setRequestedVariant(osDef.variants[0]);
                      }
                    }}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  >
                    {(OS_OPTIONS[cloud] || []).map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <label htmlFor="tester-variant" className="block text-xs text-gray-400 mb-1">
                    Variant
                  </label>
                  <select
                    id="tester-variant"
                    value={requestedVariant}
                    onChange={(e) => setRequestedVariant(e.target.value)}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  >
                    {((OS_OPTIONS[cloud] || []).find((o) => o.value === requestedOs)?.variants ?? ['server']).map((v) => (
                      <option key={v} value={v}>
                        {v === 'server' ? 'Server' : v === 'desktop' ? 'Desktop' : v}
                      </option>
                    ))}
                  </select>
                </div>
              </div>

              {/* VM size */}
              <div>
                <label htmlFor="tester-vmsize" className="block text-xs text-gray-400 mb-1">
                  {cloud === 'aws' ? 'Instance type' : cloud === 'gcp' ? 'Machine type' : 'VM size'}
                </label>
                <select
                  id="tester-vmsize"
                  value={vmSize}
                  onChange={(e) => setVmSize(e.target.value)}
                  className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                >
                  {(VM_SIZE_PRESETS[cloud] || VM_SIZE_PRESETS.azure).map((p) => (
                    <option key={p.value} value={p.value}>
                      {p.label}
                    </option>
                  ))}
                </select>
              </div>

              {/* Auto-shutdown */}
              <div className="border border-gray-800 rounded p-3 space-y-2">
                <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={autoShutdownEnabled}
                    onChange={(e) => setAutoShutdownEnabled(e.target.checked)}
                    className="accent-cyan-500"
                  />
                  Auto-shutdown enabled
                </label>
                <div>
                  <label htmlFor="tester-shutdown-hour" className="block text-xs text-gray-400 mb-1">
                    Local shutdown hour (0-23)
                  </label>
                  <select
                    id="tester-shutdown-hour"
                    value={autoShutdownHour}
                    disabled={!autoShutdownEnabled}
                    onChange={(e) => setAutoShutdownHour(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 disabled:opacity-50"
                  >
                    {HOURS.map((h) => (
                      <option key={h} value={h}>
                        {String(h).padStart(2, '0')}:00
                      </option>
                    ))}
                  </select>
                </div>
                <p className="text-xs text-gray-500">
                  Runner stops automatically each day at this local time
                  (region timezone). Costs drop by roughly 2/3 with an overnight
                  schedule.
                </p>
              </div>

              {/* Auto-probe */}
              <div className="border border-gray-800 rounded p-3">
                <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={autoProbeEnabled}
                    onChange={(e) => setAutoProbeEnabled(e.target.checked)}
                    className="accent-cyan-500"
                  />
                  Auto-probe on error
                </label>
                <p className="text-xs text-gray-500 mt-1" title="When enabled, the dashboard probes this runner's SSH port on a short interval whenever it enters the error state and auto-clears transient faults.">
                  When enabled, the dashboard probes this runner automatically if
                  it enters an error state. Off by default — you'll be asked to
                  run a probe manually from the detail drawer.
                </p>
              </div>

              <div className="flex justify-end gap-3 pt-4 border-t border-gray-800/50 mt-6">
                <button
                  type="button"
                  onClick={onClose}
                  className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
                >
                  Cancel
                </button>
                <button
                  type="submit"
                  disabled={stage === 'creating' || !name || !region}
                  className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
                >
                  Create Runner
                </button>
              </div>
            </form>
          )}
        </div>
      </div>
    </div>
  );
}
