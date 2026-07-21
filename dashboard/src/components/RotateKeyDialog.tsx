import { useEffect, useState } from 'react';
import { errorMessage } from '../api/client';
import { testersApi, type RotateKeyResponse } from '../api/testers';

interface RotateKeyDialogProps {
  projectId: string;
  testerId: string;
  testerName: string;
  onClose: () => void;
  onRotated?: (result: RotateKeyResponse) => void;
}

/**
 * Two-stage agent api-key rotation dialog (V044). Stage 1 is a confirmation
 * (rotation is destructive — the old key dies instantly and the agent must
 * reconnect). Stage 2 shows the new plaintext key ONCE with a copy button and a
 * warning that it is never shown again — the same show-once pattern as
 * {@link ShareDialog}. Operator-gated at the call site.
 */
export function RotateKeyDialog({
  projectId,
  testerId,
  testerName,
  onClose,
  onRotated,
}: RotateKeyDialogProps) {
  const [rotating, setRotating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<RotateKeyResponse | null>(null);
  const [copied, setCopied] = useState(false);

  const handleRotate = async () => {
    setRotating(true);
    setError(null);
    try {
      const data = await testersApi.rotateKey(projectId, testerId);
      setResult(data);
      onRotated?.(data);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRotating(false);
    }
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  const handleCopy = async () => {
    if (result?.api_key) {
      await navigator.clipboard.writeText(result.api_key);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="rotate-key-dialog-title"
        className="bg-[var(--bg-surface)] border border-gray-800 rounded-lg w-full max-w-md p-6"
        onClick={(e) => e.stopPropagation()}
      >
        <h3
          id="rotate-key-dialog-title"
          className="text-gray-100 font-bold text-lg mb-4"
        >
          Rotate agent key
        </h3>

        {!result ? (
          <>
            <div className="space-y-4">
              <p className="text-sm text-gray-300">
                Generate a new api-key for{' '}
                <span className="font-mono text-gray-100">{testerName}</span>.
              </p>
              <div className="bg-yellow-500/10 border border-yellow-500/30 rounded p-3">
                <p className="text-xs text-yellow-400">
                  The current key stops working immediately and the runner will
                  drop its connection until it reconnects with the new key. The
                  new key is shown only once.
                </p>
              </div>

              {error && <p className="text-xs text-red-400">{error}</p>}
            </div>

            <div className="flex justify-end gap-3 mt-6">
              <button
                onClick={onClose}
                className="px-4 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors"
              >
                Cancel
              </button>
              <button
                onClick={handleRotate}
                disabled={rotating}
                className="px-4 py-2 text-sm bg-cyan-600 hover:bg-cyan-500 text-white rounded transition-colors disabled:opacity-50"
              >
                {rotating ? 'Rotating...' : 'Rotate key'}
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="space-y-4">
              <div>
                <label className="block text-xs text-gray-500 mb-1">
                  New agent api-key
                </label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    readOnly
                    value={result.api_key}
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
                  This key will not be shown again. Copy it now and update the
                  runner's configuration.
                </p>
              </div>
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
