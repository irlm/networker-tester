import { useState } from 'react';
import { api, errorMessage } from '../api/client';
import type { SdkEndpointCreate } from '../api/types';
import { useToast } from '../hooks/useToast';
import { isAbsoluteHttpUrl } from '../lib/url';
import { Button } from './common/Button';
import { FormField, Input } from './common/FormControls';
import { Modal } from './common/Modal';

interface CreateSdkEndpointDialogProps {
  projectId: string;
  onClose: () => void;
  onCreated: () => void;
  initialValues?: Partial<SdkEndpointCreate>;
}

/** Default probe route mounted by the LagHound SDK. Matches the tester default. */
const DEFAULT_ROUTE = '/laghound/echo';

/**
 * Register a LagHound SDK endpoint. Slide-over form matching
 * CreateTlsProfileDialog. The LagHound token is a write-only password field —
 * it is sent on create and never displayed again (reads mask it as '********').
 */
export function CreateSdkEndpointDialog({
  projectId,
  onClose,
  onCreated,
  initialValues,
}: CreateSdkEndpointDialogProps) {
  const [name, setName] = useState(initialValues?.name ?? '');
  const [url, setUrl] = useState(initialValues?.url ?? '');
  const [token, setToken] = useState(initialValues?.token ?? '');
  const [route, setRoute] = useState(initialValues?.route ?? DEFAULT_ROUTE);
  const [description, setDescription] = useState(initialValues?.description ?? '');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const addToast = useToast();

  // Client-side validation so the user gets an inline message before the POST.
  const trimmedName = name.trim();
  const trimmedUrl = url.trim();
  const trimmedRoute = route.trim();
  const nameValid = trimmedName.length > 0;
  const urlValid = isAbsoluteHttpUrl(trimmedUrl);
  const tokenValid = token.trim().length > 0;
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
        token: token.trim(),
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
    <Modal onClose={onClose} labelledBy="create-sdk-endpoint-title" variant="slide-over">
      <form onSubmit={handleSubmit} className="p-4 md:p-6" noValidate>
          <div className="flex items-center justify-between mb-2">
            <h3 id="create-sdk-endpoint-title" className="text-lg font-bold text-gray-100">Register SDK endpoint</h3>
            <Button variant="ghost" size="xs" onClick={onClose} aria-label="Close">&#x2715;</Button>
          </div>
          <p className="text-xs text-gray-400 mb-6">
            Point LagHound at a URL that mounts the SDK routes. Probes run the{' '}
            <span className="text-cyan-400">sdkprobe</span> mode and split latency into network versus application time.
          </p>

          {error && <div className="alert alert-error mb-4 text-sm text-red-300" role="alert">{error}</div>}

          <FormField label="Name" htmlFor="sdk-name" required className="mb-4">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="Checkout API (prod)"
              required
            />
          </FormField>

          <FormField
            label="Target URL"
            htmlFor="sdk-url"
            required
            error={trimmedUrl && !urlValid ? 'Must be an absolute http(s) URL.' : undefined}
            className="mb-4"
          >
            <Input
              type="url"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://api.customer.com"
              required
            />
          </FormField>

          <FormField
            label={<>LagHound token <span className="text-faint">(write-only)</span></>}
            htmlFor="sdk-token"
            required
            hint="Encrypted at rest and never shown again. Token rotation is not available on an existing registration yet."
            error={token.length > 0 && !tokenValid ? 'Token cannot contain only whitespace.' : undefined}
            className="mb-4"
          >
            <Input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              autoComplete="new-password"
              placeholder="Sent as X-LagHound-Token"
              required
            />
          </FormField>

          <FormField
            label={<>Probe route <span className="text-faint">(optional)</span></>}
            htmlFor="sdk-route"
            error={!routeValid ? "Must be an absolute path beginning with '/'." : undefined}
            className="mb-4"
          >
            <Input
              value={route}
              onChange={(e) => setRoute(e.target.value)}
              placeholder={DEFAULT_ROUTE}
            />
          </FormField>

          <FormField
            label={<>Description <span className="text-faint">(optional)</span></>}
            htmlFor="sdk-desc"
            className="mb-6"
          >
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Production checkout service"
            />
          </FormField>

          <div className="flex justify-end gap-3 pt-4 border-t border-gray-800/50">
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={!canSubmit} loading={loading} loadingLabel="Registering…">
              Register endpoint
            </Button>
          </div>
      </form>
    </Modal>
  );
}
