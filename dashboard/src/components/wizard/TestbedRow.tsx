import type { TestbedState } from './testbed-constants';
import type { CloudAccountSummary } from '../../api/types';
import { CloudAccountCombobox } from './CloudAccountCombobox';
import {
  REGIONS,
  TOPOLOGIES,
  INSTANCE_TYPES,
  defaultInstanceType,
  LINUX_PROXIES,
  windowsProxiesFor,
  PROXY_LABELS,
  TESTER_OS_OPTIONS,
} from './testbed-constants';

// ── Props ──────────────────────────────────────────────────────────────

export interface TestbedRowProps {
  testbed: TestbedState;
  index: number;
  /** Project ID — used for the "+ add cloud account" link target. */
  projectId: string;
  /** All cloud accounts available to this project. */
  cloudAccounts: CloudAccountSummary[];
  onUpdate: (key: number, patch: Partial<TestbedState>) => void;
  onRemove: (key: number) => void;
  /** Hide the "Runner VM" picker — used by the deploy wizard where there's no tester VM to pick. */
  hideTesterOs?: boolean;
}

// ── Helpers ────────────────────────────────────────────────────────────

function providerToCloud(provider: string): string {
  const p = provider.toLowerCase();
  if (p === 'azure') return 'Azure';
  if (p === 'aws') return 'AWS';
  if (p === 'gcp') return 'GCP';
  return 'Azure';
}

// ── Component ──────────────────────────────────────────────────────────

export function TestbedRow({
  testbed,
  index,
  projectId,
  cloudAccounts,
  onUpdate,
  onRemove,
  hideTesterOs,
}: TestbedRowProps) {
  // When the user picks a card we set both cloudAccountId AND cloud, so the
  // region/proxy/vm-size lookups in updateTestbedState pick up the right
  // provider on the next update.
  const selectAccount = (acct: CloudAccountSummary) => {
    const cloud = providerToCloud(acct.provider);
    const validRegion = (REGIONS[cloud] ?? []).includes(testbed.region)
      ? testbed.region
      : (acct.region_default && (REGIONS[cloud] ?? []).includes(acct.region_default)
          ? acct.region_default
          : (REGIONS[cloud] ?? [])[0] ?? '');
    // Re-validate the SKU against the new provider; SKU lists don't overlap.
    const validSku = (INSTANCE_TYPES[cloud] ?? []).some(t => t.id === testbed.vmSize)
      ? testbed.vmSize
      : defaultInstanceType(cloud);
    onUpdate(testbed.key, {
      cloud,
      cloudAccountId: acct.account_id,
      region: validRegion,
      vmSize: validSku,
    });
  };

  return (
    <div className="border border-gray-800 p-3">
      {/* ── Row 1: Cloud account combobox ──────────────────────────────── */}
      <div className="flex items-center gap-2 mb-2">
        <span className="text-[10px] font-mono text-gray-600 w-3">{index + 1}</span>
        <span className="text-[11px] text-gray-500">Cloud account</span>
        <button
          onClick={() => onRemove(testbed.key)}
          className="text-[11px] text-gray-600 hover:text-red-400 transition-colors ml-auto"
        >
          remove
        </button>
      </div>

      <CloudAccountCombobox
        projectId={projectId}
        cloudAccounts={cloudAccounts}
        selectedAccountId={testbed.cloudAccountId}
        onSelect={selectAccount}
      />

      {/* ── Row 2: Region / Topology / Size / OS ───────────────────────── */}
      <div className="mt-3 flex items-center gap-2 flex-wrap">
        {/* Region dropdown */}
        <select
          value={testbed.region}
          onChange={e => onUpdate(testbed.key, { region: e.target.value })}
          className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {(REGIONS[testbed.cloud] ?? []).map(r => <option key={r} value={r}>{r}</option>)}
        </select>

        {/* Topology dropdown */}
        <select
          value={testbed.topology}
          onChange={e => onUpdate(testbed.key, { topology: e.target.value })}
          className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs text-gray-500 focus:outline-none focus:border-cyan-500"
        >
          {TOPOLOGIES.map(t => <option key={t} value={t}>{t}</option>)}
        </select>

        {/* Instance type dropdown — cloud-native SKUs with vCPU/RAM hint */}
        <select
          value={testbed.vmSize}
          onChange={e => onUpdate(testbed.key, { vmSize: e.target.value })}
          title="Instance type"
          className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs text-gray-300 font-mono focus:outline-none focus:border-cyan-500"
        >
          {(INSTANCE_TYPES[testbed.cloud] ?? []).map(t => (
            <option key={t.id} value={t.id}>{t.id} · {t.hint}</option>
          ))}
        </select>

        {/* OS toggle buttons */}
        <div className="flex">
          {(['linux', 'windows'] as const).map(os => (
            <button
              key={os}
              onClick={() => onUpdate(testbed.key, { os })}
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
      </div>

      {/* ── Existing VM checkbox ───────────────────────────────────────── */}
      <div className="mt-2">
        <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
          <input
            type="checkbox"
            checked={testbed.existingVm}
            onChange={e => onUpdate(testbed.key, { existingVm: e.target.checked })}
            className="accent-cyan-400"
          />
          Use existing VM
        </label>
        {testbed.existingVm && (
          <input
            type="text"
            value={testbed.existingVmId}
            onChange={e => onUpdate(testbed.key, { existingVmId: e.target.value })}
            placeholder="VM ID or IP from catalog"
            className="mt-1 bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
        )}
      </div>

      {/* ── Reverse Proxies toggle buttons ─────────────────────────────── */}
      <div className="mt-3">
        <label className="block text-xs text-gray-500 mb-1.5">Reverse Proxies</label>
        <div className="flex flex-wrap gap-2">
          {(testbed.os === 'windows'
            ? windowsProxiesFor(testbed.cloud)
            : LINUX_PROXIES
          ).map(p => (
            <button
              key={p}
              type="button"
              onClick={() => {
                const current = testbed.proxies;
                const next = current.includes(p)
                  ? current.filter(x => x !== p)
                  : [...current, p];
                onUpdate(testbed.key, { proxies: next });
              }}
              className={`px-2.5 py-1 text-xs border transition-colors ${
                testbed.proxies.includes(p)
                  ? 'bg-cyan-900/40 border-cyan-700 text-cyan-300'
                  : 'border-gray-700 text-gray-400 hover:border-gray-600'
              }`}
            >
              {PROXY_LABELS[p]}
            </button>
          ))}
        </div>
        {testbed.os === 'windows' && testbed.cloud !== 'Azure' && (
          <p className="text-xs text-yellow-500 mt-1">
            Windows endpoint deploy is not yet supported on {testbed.cloud}.
            Switch to Azure for full proxy support, or pick Linux.
          </p>
        )}
        {testbed.proxies.length === 0 && (
          (testbed.os === 'linux' || windowsProxiesFor(testbed.cloud).length > 0) && (
            <p className="text-xs text-yellow-500 mt-1">At least one proxy is required</p>
          )
        )}
      </div>

      {/* ── Tester VM type toggle buttons (benchmark wizards only) ─────── */}
      {!hideTesterOs && (
        <div className="mt-3">
          <label className="block text-xs text-gray-500 mb-1.5">Runner VM</label>
          <div className="flex gap-2">
            {TESTER_OS_OPTIONS.map(opt => (
              <button
                key={opt.id}
                type="button"
                onClick={() => onUpdate(testbed.key, { testerOs: opt.id })}
                className={`px-2.5 py-1 text-xs border transition-colors ${
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
      )}
    </div>
  );
}
