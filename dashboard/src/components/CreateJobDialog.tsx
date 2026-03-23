import { useState, useEffect, useRef, useCallback } from 'react';
import { api } from '../api/client';
import type { ModeGroup, Deployment, Agent } from '../api/types';
import { THROUGHPUT_IDS } from '../lib/chart';
import { ModeSelector } from './common/ModeSelector';
import { PayloadSelector } from './common/PayloadSelector';
import { useToast } from '../hooks/useToast';

interface CreateJobDialogProps {
  projectId: string;
  onClose: () => void;
  onCreated: () => void;
}

export function CreateJobDialog({ projectId, onClose, onCreated }: CreateJobDialogProps) {
  const [target, setTarget] = useState('https://localhost:8443/health');
  const [selectedModes, setSelectedModes] = useState<Set<string>>(
    new Set<string>()
  );
  const [runs, setRuns] = useState(3);
  const [concurrency, setConcurrency] = useState(1);
  const [timeout, setTimeout_] = useState(30);
  const [insecure, setInsecure] = useState(true);
  const [connectionReuse, setConnectionReuse] = useState(false);
  const [captureMode, setCaptureMode] = useState<'none' | 'tester' | 'endpoint' | 'both'>('none');
  const [payloadSizes, setPayloadSizes] = useState<Set<string>>(new Set(['64k', '1m']));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [modeGroups, setModeGroups] = useState<ModeGroup[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [testers, setTesters] = useState<Agent[]>([]);
  const [selectedTester, setSelectedTester] = useState<string>('');
  const dialogRef = useRef<HTMLDivElement>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const addToast = useToast();

  const needsPayload = THROUGHPUT_IDS.some((m) => selectedModes.has(m));

  // Track health status per deployment: true=online, false=offline, undefined=checking
  const [endpointHealth, setEndpointHealth] = useState<Record<string, boolean | undefined>>({});

  useEffect(() => {
    firstInputRef.current?.focus();
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
    api.getDeployments(projectId, { limit: 20 }).then(deps => {
      const completed = deps.filter(d => d.status === 'completed' && d.endpoint_ips && d.endpoint_ips.length > 0);
      setDeployments(completed);
      // Check health for each deployment
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

  const selectPreset = (preset: 'clear' | 'quick' | 'standard' | 'full') => {
    switch (preset) {
      case 'clear':
        setSelectedModes(new Set());
        setRuns(3);
        break;
      case 'quick':
        setSelectedModes(new Set(['http1', 'http2']));
        setRuns(1);
        break;
      case 'standard':
        setSelectedModes(new Set(['tcp', 'http1', 'http2', 'http3', 'dns', 'tls']));
        setRuns(3);
        break;
      case 'full':
        // Select ALL modes from the API
        setSelectedModes(new Set(modeGroups.flatMap(g => g.modes.map(m => m.id))));
        setRuns(5);
        setPayloadSizes(new Set(['64k', '1m', '16m']));
        break;
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (selectedModes.size === 0) {
      setError('Select at least one mode');
      return;
    }
    setLoading(true);
    setError(null);
    try {
      // Ensure target has a scheme — tester requires a full URL
      const normalizedTarget = target.match(/^https?:\/\//) ? target : `https://${target}`;
      const result = await api.createJob(projectId, {
        target: normalizedTarget,
        modes: Array.from(selectedModes),
        runs,
        concurrency,
        timeout_secs: timeout,
        payload_sizes: needsPayload ? Array.from(payloadSizes) : [],
        insecure,
        dns_enabled: true,
        connection_reuse: connectionReuse,
        ...(captureMode !== 'none' ? { capture_mode: captureMode } : {}),
      }, selectedTester || undefined);
      addToast('success', `Test ${result.job_id.slice(0, 8)} created`);
      onCreated();
      onClose();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to create job';
      setError(msg);
      addToast('error', msg);
    } finally {
      setLoading(false);
    }
  };

  const titleId = 'create-job-dialog-title';

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      {/* Backdrop — semi-transparent, click to close */}
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />

      {/* Slide-over panel */}
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <form
          onSubmit={handleSubmit}
          className="p-4 md:p-6"
        >
          <div className="flex items-center justify-between mb-6">
            <h3 id={titleId} className="text-lg font-bold text-gray-100">
              New Test
            </h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">&#x2715;</button>
          </div>

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
              <p className="text-red-400 text-sm">{error}</p>
            </div>
          )}

          {/* Presets */}
          <div className="flex gap-2 mb-4">
            <span className="text-xs text-gray-500 self-center mr-1">Preset:</span>
            {(['clear', 'quick', 'standard', 'full'] as const).map((p) => (
              <button
                key={p}
                type="button"
                onClick={() => selectPreset(p)}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 transition-colors"
              >
                {p.charAt(0).toUpperCase() + p.slice(1)}
              </button>
            ))}
          </div>

          {/* Target URL */}
          <label htmlFor="create-job-target" className="block text-xs text-gray-400 mb-1">
            Target
          </label>
          <select
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-2 focus:outline-none focus:border-cyan-500"
          >
            <option value="">Select endpoint...</option>
            <option value="https://localhost:8443/health">Local endpoint (localhost:8443)</option>
            {deployments.flatMap(d => {
              const health = endpointHealth[d.deployment_id];
              const status = health === undefined ? '...' : health ? '\u2714' : '\u2716 offline';
              // Extract OS from config if available
              const cfg = d.config as unknown as Record<string, unknown> | undefined;
              const cfgEndpoints = (cfg?.endpoints as Record<string, unknown>[] | undefined) || [];
              const osHint = cfgEndpoints.length > 0
                ? (() => {
                    const ep = cfgEndpoints[0];
                    // Config shape: { azure: { os: 'linux' } } or { os: 'linux' }
                    const nested = (ep.azure || ep.aws || ep.gcp) as Record<string, unknown> | undefined;
                    const os = (nested?.os || ep.os || '') as string;
                    return os === 'windows' ? 'Win' : os === 'linux' ? 'Ubuntu' : '';
                  })()
                : '';
              return (d.endpoint_ips || []).map(ip => (
                <option key={`${d.deployment_id}-${ip}`} value={`https://${ip}:8443/health`}>
                  {d.name}{osHint && !d.name.includes(osHint) ? ` ${osHint}` : ''} [{status}]
                </option>
              ));
            })}
            {target && !['', 'https://localhost:8443/health'].includes(target) &&
              !deployments.some(d => (d.endpoint_ips || []).some(ip => target.includes(ip))) && (
              <option value={target}>Custom: {target}</option>
            )}
          </select>
          {(() => {
            const selectedDep = deployments.find(d => (d.endpoint_ips || []).some(ip => target.includes(ip)));
            if (!selectedDep) return null;
            const health = endpointHealth[selectedDep.deployment_id];
            if (health === undefined) return (
              <p className="text-xs text-gray-500 mt-1 mb-1">Checking endpoint health...</p>
            );
            if (health === false) return (
              <p className="text-xs text-red-400 mt-1 mb-1">
                Endpoint is offline — start the VM from Settings before running a test
              </p>
            );
            return (
              <p className="text-xs text-green-400 mt-1 mb-1">Endpoint is online</p>
            );
          })()}
          <input
            ref={firstInputRef}
            id="create-job-target"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="Or type a custom URL..."
            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-400 mb-4 focus:outline-none focus:border-cyan-500"
          />

          {/* Tester Selection */}
          {testers.length > 0 && (
            <div className="mb-4">
              <label htmlFor="create-job-tester" className="block text-xs text-gray-400 mb-1">
                Tester
              </label>
              <select
                id="create-job-tester"
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

          {/* Payload Sizes (shown when throughput modes selected) */}
          {needsPayload && (
            <div className="mb-4">
              <p className="text-xs text-gray-400 mb-2">Payload Sizes</p>
              <PayloadSelector selected={payloadSizes} onToggle={togglePayload} />
            </div>
          )}

          {/* Settings Row */}
          <div className="grid grid-cols-3 gap-3 mb-4">
            <div>
              <label htmlFor="create-job-runs" className="block text-xs text-gray-400 mb-1">
                Runs
              </label>
              <input
                id="create-job-runs"
                type="number"
                min={1}
                max={100}
                value={runs}
                onChange={(e) => setRuns(Number(e.target.value))}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div>
              <label htmlFor="create-job-concurrency" className="block text-xs text-gray-400 mb-1">
                Concurrency
              </label>
              <input
                id="create-job-concurrency"
                type="number"
                min={1}
                max={50}
                value={concurrency}
                onChange={(e) => setConcurrency(Number(e.target.value))}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div>
              <label htmlFor="create-job-timeout" className="block text-xs text-gray-400 mb-1">
                Timeout (sec)
              </label>
              <input
                id="create-job-timeout"
                type="number"
                min={1}
                max={300}
                value={timeout}
                onChange={(e) => setTimeout_(Number(e.target.value))}
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
          </div>

          {/* Options Row */}
          <div className="flex gap-6 mb-4">
            <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
              <input
                type="checkbox"
                checked={insecure}
                onChange={(e) => setInsecure(e.target.checked)}
                className="accent-cyan-500"
              />
              Skip TLS verify
            </label>
            <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
              <input
                type="checkbox"
                checked={connectionReuse}
                onChange={(e) => setConnectionReuse(e.target.checked)}
                className="accent-cyan-500"
              />
              Connection reuse
            </label>
          </div>

          {/* Packet Capture */}
          <div className="mb-4">
            <label className="block text-xs text-gray-400 mb-1">Packet Capture</label>
            <select
              value={captureMode}
              onChange={(e) => setCaptureMode(e.target.value as 'none' | 'tester')}
              className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-2 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
            >
              <option value="none">Disabled</option>
              <option value="tester">Tester-side (capture on this machine)</option>
            </select>
            {captureMode !== 'none' && (
              <p className="text-xs text-yellow-400/70 mt-1">Captures network packets during the test (requires tshark on the tester)</p>
            )}
          </div>

          {/* Summary */}
          <div className="bg-[var(--bg-base)] border border-gray-800 rounded p-3 mb-4 text-xs text-gray-500">
            <span className="text-gray-400">{selectedModes.size} modes</span>
            {' · '}
            <span>{runs} runs</span>
            {needsPayload && (
              <>{' · '}<span>{payloadSizes.size} payload sizes</span></>
            )}
            {' · '}
            <span>{concurrency} concurrent</span>
            {' · '}
            <span>{timeout}s timeout</span>
          </div>

          {/* Actions — sticky bottom */}
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
              disabled={loading || selectedModes.size === 0}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
            >
              {loading ? 'Creating...' : 'Run Test'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
