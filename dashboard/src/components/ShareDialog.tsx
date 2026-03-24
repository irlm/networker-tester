import { useState } from 'react';
import { api } from '../api/client';

interface ShareDialogProps {
  projectId: string;
  resourceType: string;
  resourceId: string;
  onClose: () => void;
  onCreated?: () => void;
}

export function ShareDialog({ projectId, resourceType, resourceId, onClose, onCreated }: ShareDialogProps) {
  const [label, setLabel] = useState('');
  const [expiresInDays, setExpiresInDays] = useState(30);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<{ url: string; expires_at: string } | null>(null);
  const [copied, setCopied] = useState(false);

  const handleCreate = async () => {
    setCreating(true);
    setError(null);
    try {
      const data = await api.createShareLink(projectId, {
        resource_type: resourceType,
        resource_id: resourceId,
        label: label || undefined,
        expires_in_days: expiresInDays,
      });
      setResult(data);
      onCreated?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  const handleCopy = async () => {
    if (result?.url) {
      await navigator.clipboard.writeText(result.url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        className="bg-[var(--bg-surface)] border border-gray-800 rounded-lg w-full max-w-md p-6 shadow-xl"
        onClick={e => e.stopPropagation()}
      >
        <h3 className="text-gray-100 font-bold text-lg mb-4">Create Share Link</h3>

        {!result ? (
          <>
            <div className="space-y-4">
              <div>
                <label className="block text-xs text-gray-500 mb-1">Label (optional)</label>
                <input
                  type="text"
                  value={label}
                  onChange={e => setLabel(e.target.value)}
                  placeholder="e.g. Q1 report for client"
                  className="w-full bg-[var(--bg-base)] border border-gray-800 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-gray-600"
                />
              </div>

              <div>
                <label className="block text-xs text-gray-500 mb-1">Expires in</label>
                <select
                  value={expiresInDays}
                  onChange={e => setExpiresInDays(Number(e.target.value))}
                  className="w-full bg-[var(--bg-base)] border border-gray-800 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-gray-600"
                >
                  <option value={7}>7 days</option>
                  <option value={30}>30 days</option>
                  <option value={90}>90 days</option>
                  <option value={365}>365 days</option>
                </select>
              </div>

              <p className="text-xs text-gray-600">
                Sharing: {resourceType} {resourceId.slice(0, 8)}
              </p>

              {error && (
                <p className="text-xs text-red-400">{error}</p>
              )}
            </div>

            <div className="flex justify-end gap-3 mt-6">
              <button
                onClick={onClose}
                className="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={handleCreate}
                disabled={creating}
                className="px-4 py-2 text-sm bg-cyan-600 hover:bg-cyan-500 text-white rounded transition-colors disabled:opacity-50"
              >
                {creating ? 'Creating...' : 'Create Link'}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="space-y-4">
              <div>
                <label className="block text-xs text-gray-500 mb-1">Share URL</label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    readOnly
                    value={result.url}
                    className="flex-1 bg-[var(--bg-base)] border border-gray-800 rounded px-3 py-2 text-sm text-gray-200 font-mono"
                  />
                  <button
                    onClick={handleCopy}
                    className="px-3 py-2 text-sm bg-gray-800 hover:bg-gray-700 text-gray-200 rounded transition-colors whitespace-nowrap"
                  >
                    {copied ? 'Copied!' : 'Copy'}
                  </button>
                </div>
              </div>

              <div className="bg-yellow-500/10 border border-yellow-500/30 rounded p-3">
                <p className="text-xs text-yellow-400">
                  This link will not be shown again. Copy it now.
                </p>
              </div>

              <p className="text-xs text-gray-600">
                Expires: {new Date(result.expires_at).toLocaleDateString()}
              </p>
            </div>

            <div className="flex justify-end mt-6">
              <button
                onClick={onClose}
                className="px-4 py-2 text-sm bg-gray-800 hover:bg-gray-700 text-gray-200 rounded transition-colors"
              >
                Done
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
