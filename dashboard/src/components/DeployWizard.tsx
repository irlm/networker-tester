import { useState, useEffect, useRef, useCallback } from 'react';
import { api } from '../api/client';
import type { CloudStatus, CloudConnection, DeployEndpoint, ModeGroup } from '../api/types';
import { THROUGHPUT_IDS } from '../lib/chart';
import { ModeSelector } from './common/ModeSelector';
import { PayloadSelector } from './common/PayloadSelector';
import { useToast } from '../hooks/useToast';

interface DeployWizardProps {
  projectId: string;
  onClose: () => void;
  onCreated: (deploymentId: string) => void;
}

const AZURE_REGIONS = ['eastus', 'westus2', 'westeurope', 'northeurope', 'southeastasia'];
const AWS_REGIONS = ['us-east-1', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-1'];
const GCP_REGIONS = ['us-central1', 'us-east1', 'europe-west1', 'europe-west4', 'asia-southeast1'];

const AZURE_SIZES = ['Standard_B2s', 'Standard_B2ms', 'Standard_D2s_v3'];
const AWS_TYPES = ['t3.small', 't3.medium', 't3.large'];
const GCP_MACHINES = ['e2-small', 'e2-medium', 'e2-standard-2'];

// Mode groups are fetched from GET /api/modes (single source of truth: Protocol enum in Rust)

function stackForOs(os: string): string[] {
  return os === 'windows' ? ['iis'] : ['nginx'];
}

function emptyEndpoint(provider = 'azure'): DeployEndpoint {
  return {
    provider,
    region: provider === 'azure' ? 'eastus' : provider === 'aws' ? 'us-east-1' : 'us-central1',
    os: 'linux',
    http_stacks: ['nginx'],
    vm_size: provider === 'azure' ? 'Standard_B2s' : undefined,
    instance_type: provider === 'aws' ? 't3.small' : undefined,
    machine_type: provider === 'gcp' ? 'e2-small' : undefined,
    zone: provider === 'gcp' ? 'us-central1-a' : undefined,
  };
}

export function DeployWizard({ projectId, onClose, onCreated }: DeployWizardProps) {
  const [step, setStep] = useState(1);
  const [cloudStatus, setCloudStatus] = useState<CloudStatus | null>(null);
  const [cloudConnections, setCloudConnections] = useState<CloudConnection[]>([]);
  const [cloudLoading, setCloudLoading] = useState(true);
  const [endpoints, setEndpoints] = useState<DeployEndpoint[]>([emptyEndpoint()]);
  const [name, setName] = useState('');
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set(['tcp', 'http1', 'http2', 'http3', 'dns', 'tls']));
  const [runs, setRuns] = useState(3);
  const [concurrency, setConcurrency] = useState(1);
  const [timeout, setTimeout_] = useState(30);
  const [insecure, setInsecure] = useState(true);
  const [payloadSizes, setPayloadSizes] = useState<Set<string>>(new Set(['64k', '1m']));
  const [endpointOnly, setEndpointOnly] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const dialogRef = useRef<HTMLDivElement>(null);
  const addToast = useToast();

  const needsPayload = THROUGHPUT_IDS.some((m) => selectedModes.has(m));

  useEffect(() => {
    api.getCloudStatus(projectId)
      .then(setCloudStatus)
      .catch(() => setCloudStatus(null))
      .finally(() => setCloudLoading(false));
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
    api.getCloudConnections(projectId)
      .then(conns => setCloudConnections(conns.filter(c => c.status === 'active')))
      .catch(() => {});
  }, [projectId]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); },
    [onClose]
  );

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // Focus trap — keep Tab within the dialog
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
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault(); last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault(); first.focus();
      }
    }
    dialog.addEventListener('keydown', trapFocus);
    return () => dialog.removeEventListener('keydown', trapFocus);
  }, []);

  const updateEndpoint = (idx: number, updates: Partial<DeployEndpoint>) => {
    setEndpoints(prev => prev.map((ep, i) => i === idx ? { ...ep, ...updates } : ep));
  };

  const addEndpoint = () => {
    if (endpoints.length < 4) {
      setEndpoints(prev => [...prev, emptyEndpoint()]);
    }
  };

  const removeEndpoint = (idx: number) => {
    if (endpoints.length > 1) {
      setEndpoints(prev => prev.filter((_, i) => i !== idx));
    }
  };

  const changeProvider = (idx: number, provider: string) => {
    const ep = emptyEndpoint(provider);
    if (provider === 'lan') {
      setEndpoints(prev => prev.map((_, i) => i === idx ? { provider: 'lan', ip: '', ssh_user: 'root', ssh_port: 22 } : _));
    } else {
      setEndpoints(prev => prev.map((_, i) => i === idx ? ep : _));
    }
  };

  const toggleMode = (id: string) => {
    setSelectedModes(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const togglePayload = (val: string) => {
    setPayloadSizes(prev => {
      const next = new Set(prev);
      if (next.has(val)) next.delete(val); else next.add(val);
      return next;
    });
  };

  const autoName = () => {
    return endpoints.map(ep => {
      if (ep.provider === 'lan') return `LAN ${ep.ip || ''}`.trim();
      const os = (ep.os || 'linux') === 'windows' ? 'Win' : 'Ubuntu';
      return `${ep.provider?.toUpperCase()} ${ep.region || ''} ${os}`.trim();
    }).join(' + ');
  };

  const handleSubmit = async () => {
    if (!endpointOnly && selectedModes.size === 0) {
      setError('Select at least one probe mode');
      return;
    }
    setLoading(true);
    setError(null);

    const deployName = name || autoName() || 'Deployment';

    // Build deploy.json matching install.sh --deploy schema
    // See docs/deploy-config.md for the full spec
    const config: Record<string, unknown> = {
      version: 1,
      tester: { provider: 'local' },
      endpoints: endpoints.map(ep => {
        const entry: Record<string, unknown> = { provider: ep.provider };
        if (ep.provider === 'lan') {
          entry.lan = {
            ip: ep.ip || '',
            user: ep.ssh_user || '',
            port: ep.ssh_port || 22,
          };
        } else if (ep.provider === 'azure') {
          const osSuffix = (ep.os || 'linux') === 'windows' ? 'win' : 'ubuntu';
          const suffix = Math.random().toString(36).slice(2, 6);
          entry.azure = {
            region: ep.region || 'eastus',
            vm_size: ep.vm_size || 'Standard_B2s',
            os: ep.os || 'linux',
            vm_name: `nwk-ep-${osSuffix}-${suffix}`,
            ...(ep.resource_group ? { resource_group: ep.resource_group } : {}),
          };
        } else if (ep.provider === 'aws') {
          const osSuffix = (ep.os || 'linux') === 'windows' ? 'win' : 'ubuntu';
          const suffix = Math.random().toString(36).slice(2, 6);
          entry.aws = {
            region: ep.region || 'us-east-1',
            instance_type: ep.instance_type || 't3.small',
            os: ep.os || 'linux',
            instance_name: `nwk-ep-${osSuffix}-${suffix}`,
          };
        } else if (ep.provider === 'gcp') {
          const osSuffix = (ep.os || 'linux') === 'windows' ? 'win' : 'ubuntu';
          const suffix = Math.random().toString(36).slice(2, 6);
          entry.gcp = {
            region: ep.region || 'us-central1',
            zone: ep.zone || 'us-central1-a',
            machine_type: ep.machine_type || 'e2-small',
            os: ep.os || 'linux',
            instance_name: `nwk-ep-${osSuffix}-${suffix}`,
          };
        }
        if (ep.label) entry.label = ep.label;
        if (ep.http_stacks && ep.http_stacks.length > 0) {
          entry.http_stacks = ep.http_stacks;
        }
        return entry;
      }),
    };

    if (endpointOnly) {
      config.tests = { run_tests: false };
    } else {
      const testsObj: Record<string, unknown> = {
        modes: Array.from(selectedModes),
        runs,
        insecure,
      };
      if (needsPayload && payloadSizes.size > 0) {
        testsObj.payload_sizes = Array.from(payloadSizes);
      }
      config.tests = testsObj;
    }

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

  const providerAvailable = (p: string) => {
    if (!cloudStatus) return false;
    const s = cloudStatus[p as keyof CloudStatus];
    return s?.available && s?.authenticated;
  };

  const titleId = 'deploy-wizard-title';

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />

      {/* Slide-over panel */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-6">
            <div className="flex items-center gap-4">
              <h3 id={titleId} className="text-lg font-bold text-gray-100">
                Deploy & Test
              </h3>
              <div className="flex gap-1">
                {(endpointOnly ? [1, 2, 3] : [1, 2, 3, 4]).map(s => (
                  <div
                    key={s}
                    className={`w-6 h-1 rounded-full ${s <= step ? 'bg-green-500' : 'bg-gray-700'}`}
                  />
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

          {/* Step 1: Cloud Status */}
          {step === 1 && (
            <div>
              <p className="text-sm text-gray-400 mb-3">Cloud provider status on this host:</p>
              {cloudLoading ? (
                <p className="text-gray-500 text-sm">Checking cloud CLIs...</p>
              ) : cloudStatus ? (
                <>
                  <div className="grid grid-cols-2 gap-3 mb-4">
                    {(['azure', 'aws', 'gcp', 'ssh'] as const).map(p => {
                      const s = cloudStatus[p];
                      return (
                        <div key={p} className="bg-[var(--bg-base)] border border-gray-800 rounded p-3">
                          <div className="flex items-center gap-2 mb-1">
                            <span className={`w-2 h-2 rounded-full ${
                              s.authenticated ? 'bg-green-400' : s.available ? 'bg-yellow-400' : 'bg-gray-600'
                            }`} />
                            <span className="text-sm text-gray-200 font-medium">
                              {p === 'ssh' ? 'SSH/LAN' : p.toUpperCase()}
                            </span>
                          </div>
                          <p className="text-xs text-gray-500">
                            {!s.available ? 'CLI not installed' :
                             !s.authenticated ? 'Not authenticated' :
                             s.account ? `Account: ${s.account}` : 'Ready'}
                          </p>
                        </div>
                      );
                    })}
                  </div>
                  {cloudConnections.length > 0 && (
                    <div className="mb-4">
                      <p className="text-xs text-gray-500 tracking-wider font-medium mb-2">cloud accounts (federation)</p>
                      <div className="space-y-1">
                        {cloudConnections.map(c => (
                          <div key={c.connection_id} className="flex items-center gap-2 text-xs">
                            <span className="w-1.5 h-1.5 rounded-full bg-green-400" />
                            <span className={
                              c.provider === 'azure' ? 'text-blue-400' :
                              c.provider === 'aws' ? 'text-orange-400' : 'text-green-400'
                            }>{c.provider.toUpperCase()}</span>
                            <span className="text-gray-300">{c.name}</span>
                          </div>
                        ))}
                      </div>
                    </div>
                  )}
                </>
              ) : (
                <p className="text-yellow-400 text-sm mb-4">Could not detect cloud CLIs. Ensure az, aws, or gcloud is installed and authenticated.</p>
              )}
            </div>
          )}

          {/* Step 2: Endpoint Config */}
          {step === 2 && (
            <div>
              <p className="text-sm text-gray-400 mb-3">Configure endpoints to deploy:</p>
              {endpoints.map((ep, idx) => (
                <div key={idx} className="bg-[var(--bg-base)] border border-gray-800 rounded p-3 mb-3">
                  <div className="flex items-center justify-between mb-2">
                    <span className="text-xs text-gray-500 font-medium">Endpoint {idx + 1}</span>
                    {endpoints.length > 1 && (
                      <button
                        type="button"
                        onClick={() => removeEndpoint(idx)}
                        className="text-xs text-gray-500 hover:text-red-400"
                      >
                        Remove
                      </button>
                    )}
                  </div>

                  <div className="grid grid-cols-2 gap-3 mb-2">
                    <div>
                      <label className="block text-xs text-gray-400 mb-1">Provider</label>
                      <select
                        value={ep.provider}
                        onChange={e => changeProvider(idx, e.target.value)}
                        className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        {cloudConnections.filter(c => c.provider === 'azure').length > 0 ? (
                          cloudConnections.filter(c => c.provider === 'azure').map(c => (
                            <option key={c.connection_id} value="azure">{c.name}</option>
                          ))
                        ) : (
                          <option value="azure" disabled={!providerAvailable('azure')}>Azure{!providerAvailable('azure') ? ' (unavailable)' : ''}</option>
                        )}
                        {cloudConnections.filter(c => c.provider === 'aws').length > 0 ? (
                          cloudConnections.filter(c => c.provider === 'aws').map(c => (
                            <option key={c.connection_id} value="aws">{c.name}</option>
                          ))
                        ) : (
                          <option value="aws" disabled={!providerAvailable('aws')}>AWS{!providerAvailable('aws') ? ' (unavailable)' : ''}</option>
                        )}
                        {cloudConnections.filter(c => c.provider === 'gcp').length > 0 ? (
                          cloudConnections.filter(c => c.provider === 'gcp').map(c => (
                            <option key={c.connection_id} value="gcp">{c.name}</option>
                          ))
                        ) : (
                          <option value="gcp" disabled={!providerAvailable('gcp')}>GCP{!providerAvailable('gcp') ? ' (unavailable)' : ''}</option>
                        )}
                        <option value="lan">LAN/SSH</option>
                      </select>
                    </div>

                    {ep.provider === 'lan' ? (
                      <div>
                        <label className="block text-xs text-gray-400 mb-1">IP Address</label>
                        <input
                          value={ep.ip || ''}
                          onChange={e => updateEndpoint(idx, { ip: e.target.value })}
                          placeholder="192.168.1.100"
                          className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                    ) : (
                      <div>
                        <label className="block text-xs text-gray-400 mb-1">Region</label>
                        <select
                          value={ep.region || ''}
                          onChange={e => updateEndpoint(idx, { region: e.target.value })}
                          className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                        >
                          {(ep.provider === 'azure' ? AZURE_REGIONS :
                            ep.provider === 'aws' ? AWS_REGIONS : GCP_REGIONS
                          ).map(r => <option key={r} value={r}>{r}</option>)}
                        </select>
                      </div>
                    )}
                  </div>

                  {ep.provider === 'lan' ? (
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-xs text-gray-400 mb-1">SSH User</label>
                        <input
                          value={ep.ssh_user || 'root'}
                          onChange={e => updateEndpoint(idx, { ssh_user: e.target.value })}
                          className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                      <div>
                        <label className="block text-xs text-gray-400 mb-1">SSH Port</label>
                        <input
                          type="number"
                          value={ep.ssh_port || 22}
                          onChange={e => updateEndpoint(idx, { ssh_port: Number(e.target.value) })}
                          className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                        />
                      </div>
                    </div>
                  ) : (
                    <div className="grid grid-cols-2 gap-3">
                      <div>
                        <label className="block text-xs text-gray-400 mb-1">
                          {ep.provider === 'azure' ? 'VM Size' : ep.provider === 'aws' ? 'Instance Type' : 'Machine Type'}
                        </label>
                        <select
                          value={ep.provider === 'azure' ? ep.vm_size : ep.provider === 'aws' ? ep.instance_type : ep.machine_type}
                          onChange={e => {
                            if (ep.provider === 'azure') updateEndpoint(idx, { vm_size: e.target.value });
                            else if (ep.provider === 'aws') updateEndpoint(idx, { instance_type: e.target.value });
                            else updateEndpoint(idx, { machine_type: e.target.value });
                          }}
                          className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                        >
                          {(ep.provider === 'azure' ? AZURE_SIZES :
                            ep.provider === 'aws' ? AWS_TYPES : GCP_MACHINES
                          ).map(s => <option key={s} value={s}>{s}</option>)}
                        </select>
                      </div>
                      <div>
                        <label className="block text-xs text-gray-400 mb-1">OS</label>
                        <select
                          value={ep.os || 'linux'}
                          onChange={e => {
                            const os = e.target.value;
                            updateEndpoint(idx, { os, http_stacks: stackForOs(os) });
                          }}
                          className="w-full bg-[var(--bg-raised)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                        >
                          <option value="linux">Linux (Ubuntu)</option>
                          <option value="windows">Windows Server</option>
                        </select>
                      </div>
                    </div>
                  )}

                  {ep.provider !== 'lan' && (() => {
                    const stack = (ep.os || 'linux') === 'windows' ? 'iis' : 'nginx';
                    const enabled = ep.http_stacks?.includes(stack) || false;
                    return (
                      <div className="mt-2">
                        <label className="block text-xs text-gray-400 mb-1">HTTP Stack</label>
                        <label
                          className={`inline-flex items-center gap-1 px-2 py-1 rounded border cursor-pointer text-xs transition-colors ${
                            enabled
                              ? 'border-cyan-500/50 bg-cyan-500/10 text-cyan-400'
                              : 'border-gray-700 text-gray-400 hover:border-gray-600'
                          }`}
                        >
                          <input
                            type="checkbox"
                            checked={enabled}
                            onChange={() => updateEndpoint(idx, { http_stacks: enabled ? [] : [stack] })}
                            className="sr-only"
                          />
                          {stack.toUpperCase()}
                          <span className="text-gray-600 ml-1">
                            ({(ep.os || 'linux') === 'windows' ? 'Windows' : 'Ubuntu'})
                          </span>
                        </label>
                      </div>
                    );
                  })()}
                </div>
              ))}

              {endpoints.length < 4 && (
                <button
                  type="button"
                  onClick={addEndpoint}
                  className="text-xs text-cyan-400 hover:text-cyan-300"
                >
                  + Add endpoint
                </button>
              )}

              <div className="mt-4 pt-3 border-t border-gray-800">
                <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={endpointOnly}
                    onChange={e => setEndpointOnly(e.target.checked)}
                    className="accent-cyan-500"
                  />
                  Deploy endpoint only (skip tests)
                </label>
                <p className="text-xs text-gray-600 mt-1 ml-5">
                  You can run a test later from the Tests page
                </p>
              </div>
            </div>
          )}

          {/* Step 3: Test Config */}
          {step === 3 && !endpointOnly && (
            <div>
              <p className="text-sm text-gray-400 mb-3">Configure test to run after deployment:</p>

              {/* Presets */}
              <div className="flex gap-2 mb-4">
                <span className="text-xs text-gray-500 self-center mr-1">Preset:</span>
                {([
                  { id: 'quick', label: 'Quick', modes: ['http1', 'http2'], r: 1 },
                  { id: 'standard', label: 'Standard', modes: ['tcp', 'http1', 'http2', 'http3', 'dns', 'tls'], r: 3 },
                  { id: 'full', label: 'Full', modes: ['tcp', 'dns', 'tls', 'http1', 'http2', 'http3', 'download', 'upload', 'pageload', 'pageload2', 'pageload3', 'browser1', 'browser2', 'browser3'], r: 5 },
                ] as const).map(p => (
                  <button
                    key={p.id}
                    type="button"
                    onClick={() => { setSelectedModes(new Set(p.modes)); setRuns(p.r); }}
                    className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 transition-colors"
                  >
                    {p.label}
                  </button>
                ))}
              </div>

              {/* Mode Selection */}
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
                  <input type="number" min={1} max={100} value={runs} onChange={e => setRuns(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500" />
                </div>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Concurrency</label>
                  <input type="number" min={1} max={50} value={concurrency} onChange={e => setConcurrency(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500" />
                </div>
                <div>
                  <label className="block text-xs text-gray-400 mb-1">Timeout (sec)</label>
                  <input type="number" min={1} max={300} value={timeout} onChange={e => setTimeout_(Number(e.target.value))}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500" />
                </div>
              </div>

              <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer mb-2">
                <input type="checkbox" checked={insecure} onChange={e => setInsecure(e.target.checked)} className="accent-cyan-500" />
                Skip TLS verify
              </label>
            </div>
          )}

          {/* Step 4: Review */}
          {step === 4 && (
            <div>
              <p className="text-sm text-gray-400 mb-3">Review deployment configuration:</p>

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
                <p className="text-xs text-gray-500 mb-2 font-medium">Endpoints ({endpoints.length})</p>
                {endpoints.map((ep, i) => (
                  <div key={i} className="text-sm text-gray-300 py-1">
                    {ep.provider === 'lan'
                      ? `LAN: ${ep.ssh_user}@${ep.ip}:${ep.ssh_port}`
                      : `${ep.provider?.toUpperCase()}: ${ep.region} (${ep.os}, ${ep.vm_size || ep.instance_type || ep.machine_type})`
                    }
                    {ep.http_stacks && ep.http_stacks.length > 0 ? (
                      <span className="text-cyan-400/60 ml-2">+ {ep.http_stacks.map(s => s.toUpperCase()).join(', ')}</span>
                    ) : (
                      <span className="text-gray-600 ml-2">(no HTTP stack)</span>
                    )}
                  </div>
                ))}
              </div>

              {endpointOnly ? (
                <div className="bg-[var(--bg-base)] border border-gray-800 rounded p-3 mb-3 text-xs text-gray-500">
                  <span className="text-gray-400">Endpoint only</span>
                  {' \u00b7 '}
                  <span>No tests will be run</span>
                </div>
              ) : (
                <div className="bg-[var(--bg-base)] border border-gray-800 rounded p-3 mb-3 text-xs text-gray-500">
                  <span className="text-gray-400">{selectedModes.size} modes</span>
                  {' \u00b7 '}
                  <span>{runs} runs</span>
                  {needsPayload && <>{' \u00b7 '}<span>{payloadSizes.size} payload sizes</span></>}
                  {' \u00b7 '}
                  <span>{concurrency} concurrent</span>
                  {' \u00b7 '}
                  <span>{timeout}s timeout</span>
                </div>
              )}
            </div>
          )}

          {/* Navigation */}
          <div className="flex justify-between pt-4 border-t border-gray-800/50 mt-6">
            <div>
              {step > 1 && (
                <button
                  type="button"
                  onClick={() => setStep(endpointOnly && step === 4 ? 2 : step - 1)}
                  className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
                >
                  Back
                </button>
              )}
            </div>
            <div className="flex gap-3">
              <button
                type="button"
                onClick={onClose}
                className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
              >
                Cancel
              </button>
              {step === 4 ? (
                <button
                  type="button"
                  onClick={handleSubmit}
                  disabled={loading}
                  className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
                >
                  {loading ? 'Deploying...' : endpointOnly ? 'Deploy Endpoint' : 'Deploy & Test'}
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => setStep(endpointOnly && step === 2 ? 4 : step + 1)}
                  className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
                >
                  Next
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
