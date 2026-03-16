import { useState, useEffect, useRef, useCallback } from 'react';
import { api } from '../api/client';
import type { ModeGroup, Deployment, Agent } from '../api/types';
import { useToast } from '../hooks/useToast';

interface CreateJobDialogProps {
  onClose: () => void;
  onCreated: () => void;
}

const PAYLOAD_PRESETS = [
  { label: 'Small (64KB)', value: '64k' },
  { label: 'Medium (1MB)', value: '1m' },
  { label: 'Large (16MB)', value: '16m' },
];

export function CreateJobDialog({ onClose, onCreated }: CreateJobDialogProps) {
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

  const THROUGHPUT_IDS = ['download', 'upload', 'download1', 'download2', 'download3',
    'upload1', 'upload2', 'upload3', 'webdownload', 'webupload', 'udpdownload', 'udpupload'];
  const needsPayload = THROUGHPUT_IDS.some((m) => selectedModes.has(m));

  useEffect(() => {
    firstInputRef.current?.focus();
    api.getModes().then(r => setModeGroups(r.groups)).catch(() => {});
    api.getDeployments({ limit: 20 }).then(deps => {
      setDeployments(deps.filter(d => d.status === 'completed' && d.endpoint_ips && d.endpoint_ips.length > 0));
    }).catch(() => {});
    api.getAgents().then(r => setTesters(r.agents)).catch(() => {});
  }, []);

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
      const result = await api.createJob({
        target,
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
      addToast('success', `Job ${result.job_id.slice(0, 8)} created`);
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
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50 p-4">
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="max-h-[90vh] overflow-y-auto"
      >
        <form
          onSubmit={handleSubmit}
          className="bg-[#12131a] border border-gray-800 rounded-lg p-6 w-[700px] min-w-[500px] max-w-[95vw] resize-x overflow-auto"
        >
          <h3 id={titleId} className="text-lg font-bold text-gray-100 mb-4">
            New Test
          </h3>

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
            className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-2 focus:outline-none focus:border-cyan-500"
          >
            <option value="">Select endpoint...</option>
            <option value="https://localhost:8443/health">Local endpoint (localhost:8443)</option>
            {deployments.flatMap(d =>
              (d.endpoint_ips || []).map(ip => (
                <option key={`${d.deployment_id}-${ip}`} value={`https://${ip}:8443/health`}>
                  {d.name} — {ip.split('.')[0]}
                </option>
              ))
            )}
            {target && !['', 'https://localhost:8443/health'].includes(target) &&
              !deployments.some(d => (d.endpoint_ips || []).some(ip => target.includes(ip))) && (
              <option value={target}>Custom: {target}</option>
            )}
          </select>
          <input
            ref={firstInputRef}
            id="create-job-target"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            placeholder="Or type a custom URL..."
            className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-1.5 text-xs text-gray-400 mb-4 focus:outline-none focus:border-cyan-500"
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
                className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
          <div className="grid grid-cols-2 gap-3 mb-4">
            {modeGroups.map((group) => (
              <div
                key={group.label}
                className={`bg-[#0a0b0f] border border-gray-800 rounded p-3 ${
                  group.label === 'Throughput' ? 'col-span-2' : ''
                }`}
              >
                {(() => {
                  const ids = group.modes.map(m => m.id);
                  const selectedCount = ids.filter(id => selectedModes.has(id)).length;
                  const allSelected = selectedCount === ids.length;
                  const someSelected = selectedCount > 0 && !allSelected;
                  return (
                    <>
                      <label className="flex items-center gap-2 mb-2 cursor-pointer">
                        <input
                          type="checkbox"
                          checked={allSelected}
                          ref={el => { if (el) el.indeterminate = someSelected; }}
                          onChange={() => {
                            setSelectedModes(prev => {
                              const next = new Set(prev);
                              ids.forEach(id => allSelected ? next.delete(id) : next.add(id));
                              return next;
                            });
                          }}
                          className="accent-cyan-500"
                        />
                        <span className="text-xs text-gray-500 font-medium">{group.label}</span>
                        {someSelected && (
                          <span className="text-[10px] text-gray-600">{selectedCount}/{ids.length}</span>
                        )}
                        {group.detail && (
                          <span className="text-gray-600 hover:text-gray-400 cursor-help ml-1 text-xs" title={group.detail}>&#9432;</span>
                        )}
                      </label>
                      <div className={`pl-5 ${group.label === 'Throughput' ? 'grid grid-cols-2 gap-x-4' : ''}`}>
                        {group.modes.map((mode) => (
                          <label
                            key={mode.id}
                            className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer py-0.5 hover:text-gray-100"
                          >
                            <input
                              type="checkbox"
                              checked={selectedModes.has(mode.id)}
                              onChange={() => toggleMode(mode.id)}
                              className="accent-cyan-500"
                            />
                            <span>{mode.name}</span>
                            <span className="text-xs text-gray-600 ml-auto" title={mode.detail}>{mode.desc}</span>
                          </label>
                        ))}
                      </div>
                    </>
                  );
                })()}
              </div>
            ))}
          </div>

          {/* Payload Sizes (shown when throughput modes selected) */}
          {needsPayload && (
            <div className="mb-4">
              <p className="text-xs text-gray-400 mb-2">Payload Sizes</p>
              <div className="flex gap-2">
                {PAYLOAD_PRESETS.map((p) => (
                  <label
                    key={p.value}
                    className={`flex items-center gap-2 px-3 py-1.5 rounded border cursor-pointer text-sm transition-colors ${
                      payloadSizes.has(p.value)
                        ? 'border-cyan-500/50 bg-cyan-500/10 text-cyan-400'
                        : 'border-gray-700 text-gray-400 hover:border-gray-600'
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={payloadSizes.has(p.value)}
                      onChange={() => togglePayload(p.value)}
                      className="sr-only"
                    />
                    {p.label}
                  </label>
                ))}
              </div>
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
                className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
                className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
                className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
              className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-2 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
            >
              <option value="none">Disabled</option>
              <option value="tester">Tester-side (capture on this machine)</option>
            </select>
            {captureMode !== 'none' && (
              <p className="text-xs text-yellow-400/70 mt-1">Requires tshark/dumpcap installed on the tester machine</p>
            )}
          </div>

          {/* Summary */}
          <div className="bg-[#0a0b0f] border border-gray-800 rounded p-3 mb-4 text-xs text-gray-500">
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

          {/* Actions */}
          <div className="flex justify-end gap-3">
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
              {loading ? 'Creating...' : 'Create Job'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
