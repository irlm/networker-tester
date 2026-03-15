import { useState, useEffect, useRef, useCallback } from 'react';
import { api } from '../api/client';

interface CreateJobDialogProps {
  onClose: () => void;
  onCreated: () => void;
}

export function CreateJobDialog({ onClose, onCreated }: CreateJobDialogProps) {
  const [target, setTarget] = useState('https://localhost:8443/health');
  const [modes, setModes] = useState('http1,http2');
  const [runs, setRuns] = useState(3);
  const [insecure, setInsecure] = useState(true);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);

  // Focus first input on mount
  useEffect(() => {
    firstInputRef.current?.focus();
  }, []);

  // Escape key handler
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    },
    [onClose]
  );

  useEffect(() => {
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // Basic focus trap
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    const focusable = dialog.querySelectorAll<HTMLElement>(
      'input, button, select, textarea, [tabindex]:not([tabindex="-1"])'
    );
    if (focusable.length === 0) return;

    const first = focusable[0];
    const last = focusable[focusable.length - 1];

    function trapFocus(e: KeyboardEvent) {
      if (e.key !== 'Tab') return;
      if (e.shiftKey) {
        if (document.activeElement === first) {
          e.preventDefault();
          last.focus();
        }
      } else {
        if (document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    }

    dialog.addEventListener('keydown', trapFocus);
    return () => dialog.removeEventListener('keydown', trapFocus);
  }, []);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    try {
      await api.createJob({
        target,
        modes: modes.split(',').map((m) => m.trim()),
        runs,
        concurrency: 1,
        timeout_secs: 30,
        payload_sizes: [],
        insecure,
        dns_enabled: true,
        connection_reuse: false,
      });
      onCreated();
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to create job');
    } finally {
      setLoading(false);
    }
  };

  const titleId = 'create-job-dialog-title';

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
      >
        <form
          onSubmit={handleSubmit}
          className="bg-[#12131a] border border-gray-800 rounded-lg p-6 w-[500px]"
        >
          <h3 id={titleId} className="text-lg font-bold text-gray-100 mb-4">
            New Test Job
          </h3>

          {error && (
            <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4">
              <p className="text-red-400 text-sm">{error}</p>
            </div>
          )}

          <label htmlFor="create-job-target" className="block text-xs text-gray-400 mb-1">
            Target URL
          </label>
          <input
            ref={firstInputRef}
            id="create-job-target"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-3 focus:outline-none focus:border-cyan-500"
          />

          <label htmlFor="create-job-modes" className="block text-xs text-gray-400 mb-1">
            Modes (comma-separated)
          </label>
          <input
            id="create-job-modes"
            value={modes}
            onChange={(e) => setModes(e.target.value)}
            className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-3 focus:outline-none focus:border-cyan-500"
          />

          <div className="flex gap-4 mb-3">
            <div className="flex-1">
              <label htmlFor="create-job-runs" className="block text-xs text-gray-400 mb-1">
                Runs
              </label>
              <input
                id="create-job-runs"
                type="number"
                value={runs}
                onChange={(e) => setRuns(Number(e.target.value))}
                className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              />
            </div>
            <div className="flex items-end pb-1">
              <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
                <input
                  type="checkbox"
                  checked={insecure}
                  onChange={(e) => setInsecure(e.target.checked)}
                  className="accent-cyan-500"
                />
                Insecure (skip TLS verify)
              </label>
            </div>
          </div>

          <div className="flex justify-end gap-3 mt-4">
            <button
              type="button"
              onClick={onClose}
              className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={loading}
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
