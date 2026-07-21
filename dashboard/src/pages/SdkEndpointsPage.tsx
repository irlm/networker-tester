import { useCallback, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, errorMessage, type SdkEndpoint } from '../api/client';
import { CreateSdkEndpointDialog } from '../components/CreateSdkEndpointDialog';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';

/**
 * LagHound SDK-endpoint management. Register a customer endpoint (target URL +
 * write-only token + optional route), list existing endpoints (token shown
 * masked), and delete with confirmation. Mutations are operator-gated; viewers
 * get a read-only list.
 */
export function SdkEndpointsPage() {
  const { projectId, isOperator } = useProject();
  const [endpoints, setEndpoints] = useState<SdkEndpoint[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<SdkEndpoint | null>(null);
  const [deleting, setDeleting] = useState(false);
  const addToast = useToast();

  usePageTitle('SDK Endpoints');

  const load = useCallback(() => {
    if (!projectId) return;
    api
      .listSdkEndpoints(projectId)
      .then((data) => {
        setEndpoints(data);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(errorMessage(e));
        setLoading(false);
      });
  }, [projectId]);

  usePolling(load, 20000);

  const handleDelete = async () => {
    if (!projectId || !confirmDelete) return;
    setDeleting(true);
    try {
      await api.deleteSdkEndpoint(projectId, confirmDelete.id);
      addToast('success', `SDK endpoint "${confirmDelete.name}" deleted`);
      setConfirmDelete(null);
      load();
    } catch (e) {
      addToast('error', errorMessage(e));
    } finally {
      setDeleting(false);
    }
  };

  if (loading && endpoints.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">SDK Endpoints</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading SDK endpoints...</div>
      </div>
    );
  }

  if (error && endpoints.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">SDK Endpoints</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load SDK endpoints</h3>
          <p className="text-red-300 text-sm">Could not fetch SDK endpoints. Check your connection and try refreshing.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="SDK Endpoints"
        subtitle="LagHound-instrumented customer endpoints probed with the sdkprobe mode."
        action={
          isOperator ? (
            <button
              onClick={() => setShowCreate(true)}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-1.5 rounded text-sm transition-colors"
            >
              + SDK endpoint
            </button>
          ) : undefined
        }
      />

      {showCreate && projectId && (
        <CreateSdkEndpointDialog projectId={projectId} onClose={() => setShowCreate(false)} onCreated={load} />
      )}

      {error && endpoints.length > 0 && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh SDK endpoints. Retrying automatically.
        </div>
      )}

      {endpoints.length === 0 ? (
        <EmptyState
          message="No SDK endpoints yet"
          detail={
            <>
              Register a URL that mounts the{' '}
              <a
                href="https://github.com/laghound"
                className="text-cyan-400 hover:underline"
                target="_blank"
                rel="noreferrer"
              >
                LagHound SDK
              </a>{' '}
              routes to measure how much of its latency is your application versus the network.
            </>
          }
          action={
            isOperator ? (
              <button
                onClick={() => setShowCreate(true)}
                className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
              >
                Register your first SDK endpoint
              </button>
            ) : undefined
          }
        />
      ) : (
        <>
          <div className="text-xs text-gray-500 mb-3">
            {endpoints.length} SDK endpoint{endpoints.length === 1 ? '' : 's'}
            {' · '}
            <Link to={`/projects/${projectId}/reports/app-network`} className="text-cyan-400 hover:underline">
              View Application Network Performance report
            </Link>
          </div>

          <div className="table-container">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="px-4 py-2.5 text-left font-medium">Name</th>
                  <th className="px-4 py-2.5 text-left font-medium">Target URL</th>
                  <th className="px-4 py-2.5 text-left font-medium">Route</th>
                  <th className="px-4 py-2.5 text-left font-medium">Token</th>
                  <th className="px-4 py-2.5 text-left font-medium">Created</th>
                  {isOperator && <th className="px-4 py-2.5 text-right font-medium">Actions</th>}
                </tr>
              </thead>
              <tbody>
                {endpoints.map((ep) => (
                  <tr key={ep.id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                    <td className="px-4 py-3 text-gray-200">
                      {ep.name}
                      {ep.description && (
                        <div className="text-xs text-gray-600 mt-0.5">{ep.description}</div>
                      )}
                    </td>
                    <td className="px-4 py-3 text-cyan-400 font-mono text-xs break-all">{ep.url ?? '—'}</td>
                    <td className="px-4 py-3 text-gray-400 font-mono text-xs">{ep.route ?? '/laghound/echo'}</td>
                    <td className="px-4 py-3 text-xs">
                      {ep.token_set ? (
                        <span className="text-gray-400 font-mono" title="Token stored (write-only)">
                          {ep.token ?? '********'}
                        </span>
                      ) : (
                        <span className="text-yellow-500">not set</span>
                      )}
                    </td>
                    <td className="px-4 py-3 text-gray-500 text-xs">{new Date(ep.created_at).toLocaleString()}</td>
                    {isOperator && (
                      <td className="px-4 py-3 text-right">
                        <button
                          onClick={() => setConfirmDelete(ep)}
                          className="text-gray-500 hover:text-red-400 text-xs transition-colors"
                          aria-label={`Delete ${ep.name}`}
                        >
                          Delete
                        </button>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {confirmDelete && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
          <div className="absolute inset-0 bg-black/50" onClick={() => setConfirmDelete(null)} aria-hidden="true" />
          <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-sdk-endpoint-title"
            className="relative bg-[var(--bg-base)] border border-gray-800 rounded-lg p-6 w-full max-w-md"
          >
            <h3 id="delete-sdk-endpoint-title" className="text-lg font-bold text-gray-100 mb-2">Delete SDK endpoint</h3>
            <p className="text-sm text-gray-400 mb-6">
              Delete <span className="text-gray-200 font-medium">{confirmDelete.name}</span>? This removes the endpoint
              and its stored token. Past probe runs and report history are kept.
            </p>
            <div className="flex justify-end gap-3">
              <button
                onClick={() => setConfirmDelete(null)}
                className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
              >
                Cancel
              </button>
              <button
                onClick={handleDelete}
                disabled={deleting}
                className="bg-red-600 hover:bg-red-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
              >
                {deleting ? 'Deleting...' : 'Delete'}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
