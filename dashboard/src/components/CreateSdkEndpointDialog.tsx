import { useEffect, useRef, useState } from 'react';
import { api, errorMessage } from '../api/client';
import { useToast } from '../hooks/useToast';

interface CreateSdkEndpointDialogProps {
  projectId: string;
  onClose: () => void;
  onCreated: () => void;
}

/** Default probe route mounted by the LagHound SDK. Matches the tester default. */
const DEFAULT_ROUTE = '/laghound/echo';

/** True for an absolute http(s) URL — mirrors the C# TryNormalizeUrl guard. */
function isAbsoluteHttpUrl(raw: string): boolean {
  try {
    const u = new URL(raw.trim());
    return u.protocol === 'http:' || u.protocol === 'https:';
  } catch {
    return false;
  }
}

/**
 * Register a LagHound SDK endpoint. Slide-over form matching
 * CreateTlsProfileDialog. The LagHound token is a write-only password field —
 * it is sent on create and never displayed again (reads mask it as '********').
 */
export function CreateSdkEndpointDialog({ projectId, onClose, onCreated }: CreateSdkEndpointDialogProps) {
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [token, setToken] = useState('');
  const [route, setRoute] = useState(DEFAULT_ROUTE);
  const [description, setDescription] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstInputRef = useRef<HTMLInputElement>(null);
  const addToast = useToast();

  useEffect(() => {
    firstInputRef.current?.focus();
  }, []);

  // Escape closes — matches every other slide-over dialog.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  // Client-side validation so the user gets an inline message before the POST.
  const trimmedName = name.trim();
  const trimmedUrl = url.trim();
  const trimmedRoute = route.trim();
  const nameValid = trimmedName.length > 0;
  const urlValid = isAbsoluteHttpUrl(trimmedUrl);
  const tokenValid = token.length > 0;
  const routeValid = trimmedRoute === '' || (trimmedRoute.startsWith('/') && !trimmedRoute.includes(' '));
  const canSubmit = nameValid && urlValid && tokenValid && routeValid && !loading;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!nameValid) return setError('Name is required.');
    if (!urlValid) return setError('Target URL must be an absolute http(s) URL.');
    if (!tokenValid) return setError('A LagHound token is required.');
    if (!routeValid) return setError("Route must be an absolute path beginning with '/'.");

    setLoading(true);
    setError(null);
    try {
      await api.createSdkEndpoint(projectId, {
        name: trimmedName,
        url: trimmedUrl,
        token,
        route: trimmedRoute || undefined,
        description: description.trim() || undefined,
      });
      addToast('success', `SDK endpoint "${trimmedName}" registered`);
      onCreated();
      onClose();
    } catch (err) {
      const msg = errorMessage(err);
      setError(msg);
      addToast('error', msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-sdk-endpoint-title"
        className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <form onSubmit={handleSubmit} className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-2">
            <h3 id="create-sdk-endpoint-title" className="text-lg font-bold text-gray-100">Register SDK endpoint</h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">&#x2715;</button>
          </div>
          <p className="text-xs text-gray-500 mb-6">
            Point LagHound at a URL that mounts the SDK routes. Probes run the{' '}
            <span className="font-mono text-purple-400">sdkprobe</span> mode and split latency into network vs server.
          </p>

          {error && <div className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4 text-red-400 text-sm">{error}</div>}

          <label htmlFor="sdk-name" className="block text-xs text-gray-400 mb-1">Name</label>
          <input
            id="sdk-name"
            ref={firstInputRef}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Checkout API (prod)"
            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-4 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />

          <label htmlFor="sdk-url" className="block text-xs text-gray-400 mb-1">Target URL</label>
          <input
            id="sdk-url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://api.customer.com"
            className={`w-full bg-[var(--bg-base)] border rounded px-3 py-2 text-sm text-gray-200 mb-1 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600 ${
              trimmedUrl && !urlValid ? 'border-red-500/50' : 'border-gray-700'
            }`}
          />
          {trimmedUrl && !urlValid && (
            <p className="text-xs text-red-400 mb-3">Must be an absolute http(s) URL.</p>
          )}
          {(!trimmedUrl || urlValid) && <div className="mb-3" />}

          <label htmlFor="sdk-token" className="block text-xs text-gray-400 mb-1">
            LagHound token <span className="text-gray-600">(write-only)</span>
          </label>
          <input
            id="sdk-token"
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            autoComplete="new-password"
            placeholder="Sent as X-LagHound-Token"
            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-1 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
          <p className="text-xs text-gray-600 mb-4">Encrypted at rest and never shown again — you can only replace it.</p>

          <label htmlFor="sdk-route" className="block text-xs text-gray-400 mb-1">
            Probe route <span className="text-gray-600">(optional)</span>
          </label>
          <input
            id="sdk-route"
            value={route}
            onChange={(e) => setRoute(e.target.value)}
            placeholder={DEFAULT_ROUTE}
            className={`w-full bg-[var(--bg-base)] border rounded px-3 py-2 text-sm text-gray-200 mb-1 font-mono focus:outline-none focus:border-cyan-500 placeholder:text-gray-600 ${
              !routeValid ? 'border-red-500/50' : 'border-gray-700'
            }`}
          />
          {!routeValid && (
            <p className="text-xs text-red-400 mb-3">Must be an absolute path beginning with &apos;/&apos;.</p>
          )}
          {routeValid && <div className="mb-3" />}

          <label htmlFor="sdk-desc" className="block text-xs text-gray-400 mb-1">
            Description <span className="text-gray-600">(optional)</span>
          </label>
          <input
            id="sdk-desc"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-6 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />

          <div className="flex justify-end gap-3 pt-4 border-t border-gray-800/50">
            <button type="button" onClick={onClose} className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200">Cancel</button>
            <button type="submit" disabled={!canSubmit} className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50">
              {loading ? 'Registering...' : 'Register endpoint'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
