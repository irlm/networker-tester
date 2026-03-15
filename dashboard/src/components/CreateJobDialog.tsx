import { useState, useEffect, useRef, useCallback } from 'react';
import { api } from '../api/client';
import { useToast } from '../hooks/useToast';

interface CreateJobDialogProps {
  onClose: () => void;
  onCreated: () => void;
}

const MODE_GROUPS = [
  {
    label: 'Network',
    modes: [
      { id: 'tcp', name: 'TCP', desc: 'TCP connect' },
      { id: 'dns', name: 'DNS', desc: 'DNS resolution' },
      { id: 'tls', name: 'TLS', desc: 'TLS handshake' },
    ],
  },
  {
    label: 'HTTP',
    modes: [
      { id: 'http1', name: 'HTTP/1.1', desc: 'HTTP/1.1 request' },
      { id: 'http2', name: 'HTTP/2', desc: 'HTTP/2 multiplexed' },
      { id: 'http3', name: 'HTTP/3', desc: 'QUIC/HTTP3' },
    ],
  },
  {
    label: 'Throughput',
    modes: [
      { id: 'download', name: 'Download', desc: 'Server→client transfer' },
      { id: 'upload', name: 'Upload', desc: 'Client→server transfer' },
    ],
  },
  {
    label: 'Page Load',
    modes: [
      { id: 'pageload1', name: 'Pageload H1', desc: '6 parallel conns' },
      { id: 'pageload2', name: 'Pageload H2', desc: 'Multiplexed TLS' },
      { id: 'pageload3', name: 'Pageload H3', desc: 'QUIC multiplexed' },
    ],
  },
];

const PAYLOAD_PRESETS = [
  { label: 'Small (64KB)', value: '64k' },
  { label: 'Medium (1MB)', value: '1m' },
  { label: 'Large (16MB)', value: '16m' },
];

export function CreateJobDialog({ onClose, onCreated }: CreateJobDialogProps) {
  const [target, setTarget] = useState('https://localhost:8443/health');
  const [selectedModes, setSelectedModes] = useState<Set<string>>(
    new Set(['http1', 'http2'])
  );
  const [runs, setRuns] = useState(3);
  const [concurrency, setConcurrency] = useState(1);
  const [timeout, setTimeout_] = useState(30);
  const [insecure, setInsecure] = useState(true);
  const [connectionReuse, setConnectionReuse] = useState(false);
  const [payloadSizes, setPayloadSizes] = useState<Set<string>>(new Set(['64k', '1m']));
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const addToast = useToast();

  const needsPayload = ['download', 'upload', 'download1', 'download2', 'download3', 'upload1', 'upload2', 'upload3']
    .some((m) => selectedModes.has(m));

  useEffect(() => { firstInputRef.current?.focus(); }, []);

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

  const selectPreset = (preset: 'quick' | 'standard' | 'full') => {
    switch (preset) {
      case 'quick':
        setSelectedModes(new Set(['http1', 'http2']));
        setRuns(1);
        break;
      case 'standard':
        setSelectedModes(new Set(['tcp', 'http1', 'http2', 'http3', 'dns', 'tls']));
        setRuns(3);
        break;
      case 'full':
        setSelectedModes(new Set(['tcp', 'dns', 'tls', 'http1', 'http2', 'http3', 'download', 'upload', 'pageload1', 'pageload2', 'pageload3']));
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
      });
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
          className="bg-[#12131a] border border-gray-800 rounded-lg p-6 w-[600px]"
        >
          <h3 id={titleId} className="text-lg font-bold text-gray-100 mb-4">
            New Test Job
          </h3>

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
              <p className="text-red-400 text-sm">{error}</p>
            </div>
          )}

          {/* Presets */}
          <div className="flex gap-2 mb-4">
            <span className="text-xs text-gray-500 self-center mr-1">Preset:</span>
            {(['quick', 'standard', 'full'] as const).map((p) => (
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
            Target URL
          </label>
          <input
            ref={firstInputRef}
            id="create-job-target"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500"
          />

          {/* Mode Selection */}
          <p className="text-xs text-gray-400 mb-2">Probe Modes</p>
          <div className="grid grid-cols-2 gap-3 mb-4">
            {MODE_GROUPS.map((group) => (
              <div key={group.label} className="bg-[#0a0b0f] border border-gray-800 rounded p-3">
                <p className="text-xs text-gray-500 font-medium mb-2">{group.label}</p>
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
                    <span className="text-xs text-gray-600 ml-auto">{mode.desc}</span>
                  </label>
                ))}
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
