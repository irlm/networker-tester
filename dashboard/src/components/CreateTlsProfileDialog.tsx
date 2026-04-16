import { useEffect, useRef, useState } from 'react';
import { api } from '../api/client';
import type { Agent } from '../api/types';
import { useToast } from '../hooks/useToast';

interface CreateTlsProfileDialogProps {
  projectId: string;
  onClose: () => void;
  onCreated: () => void;
}

export function CreateTlsProfileDialog({ projectId, onClose, onCreated }: CreateTlsProfileDialogProps) {
  const [url, setUrl] = useState('https://localhost:8443/health');
  const [ip, setIp] = useState('');
  const [sni, setSni] = useState('');
  const [targetKind, setTargetKind] = useState<'managed-endpoint' | 'external-url' | 'external-host'>('external-url');
  const [timeout, setTimeoutValue] = useState(30);
  const [insecure, setInsecure] = useState(true);
  const [testers, setTesters] = useState<Agent[]>([]);
  const [selectedTester, setSelectedTester] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const addToast = useToast();

  useEffect(() => {
    firstInputRef.current?.focus();
    api.getAgents(projectId).then(r => setTesters(r.agents)).catch(() => {});
  }, [projectId]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    try {
      const normalizedUrl = url.match(/^https?:\/\//) ? url : `https://${url}`;
      const result = await api.createJob(projectId, {
        target: normalizedUrl,
        modes: ['tls'],
        tls_profile_url: normalizedUrl,
        tls_profile_ip: ip.trim() || undefined,
        tls_profile_sni: sni.trim() || undefined,
        tls_profile_target_kind: targetKind,
        runs: 1,
        concurrency: 1,
        timeout_secs: timeout,
        payload_sizes: [],
        insecure,
        dns_enabled: true,
        connection_reuse: false,
      }, selectedTester || undefined);
      addToast('success', `TLS profile job ${result.job_id.slice(0, 8)} created`);
      onCreated();
      onClose();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to create TLS profile job';
      setError(msg);
      addToast('error', msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />
      <div className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel">
        <form onSubmit={handleSubmit} className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-6">
            <h3 className="text-lg font-bold text-gray-100">Run TLS Profile</h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">&#x2715;</button>
          </div>

          {error && <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4 text-red-400 text-sm">{error}</div>}

          <label className="block text-xs text-gray-400 mb-1">HTTPS URL</label>
          <input ref={firstInputRef} value={url} onChange={(e) => setUrl(e.target.value)} className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500" />

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
            <div>
              <label className="block text-xs text-gray-400 mb-1">IP override</label>
              <input value={ip} onChange={(e) => setIp(e.target.value)} placeholder="Optional" className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500" />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">SNI override</label>
              <input value={sni} onChange={(e) => setSni(e.target.value)} placeholder="Optional" className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500" />
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
            <div>
              <label className="block text-xs text-gray-400 mb-1">Target kind</label>
              <select value={targetKind} onChange={(e) => setTargetKind(e.target.value as 'managed-endpoint' | 'external-url' | 'external-host')} className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500">
                <option value="managed-endpoint">Managed target</option>
                <option value="external-url">External URL</option>
                <option value="external-host">External host</option>
              </select>
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Timeout (sec)</label>
              <input type="number" min={1} max={300} value={timeout} onChange={(e) => setTimeoutValue(Number(e.target.value))} className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500" />
            </div>
          </div>

          <div className="mb-4">
            <label className="block text-xs text-gray-400 mb-1">Runner</label>
            <select value={selectedTester} onChange={(e) => setSelectedTester(e.target.value)} className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500">
              <option value="">Auto (any online runner)</option>
              {testers.map((a) => <option key={a.agent_id} value={a.agent_id}>{a.name} ({a.status})</option>)}
            </select>
          </div>

          <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer mb-6">
            <input type="checkbox" checked={insecure} onChange={(e) => setInsecure(e.target.checked)} className="accent-cyan-500" />
            Skip TLS verify
          </label>

          <div className="flex justify-end gap-3 pt-4 border-t border-gray-800/50 mt-6">
            <button type="button" onClick={onClose} className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200">Cancel</button>
            <button type="submit" disabled={loading} className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50">
              {loading ? 'Creating...' : 'Run TLS Profile'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
