import { useCallback, useState } from 'react';
import { Link } from 'react-router';
import { api, errorMessage, type SdkEndpoint } from '../api/client';
import type { SdkEndpointCreate, SdkSampleStatus } from '../api/types';
import { CreateSdkEndpointDialog } from '../components/CreateSdkEndpointDialog';
import { CreateSdkSamplesDialog } from '../components/CreateSdkSamplesDialog';
import { SdkExamplesPanel } from '../components/SdkExamplesPanel';
import { SdkSamplesPanel } from '../components/SdkSamplesPanel';
import { SDK_EXAMPLES } from '../lib/sdkExamples';
import { EmptyState } from '../components/common/EmptyState';
import { DataTable, type DataTableColumn } from '../components/common/DataTable';
import { PageShell } from '../components/common/PageShell';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { Button } from '../components/common/Button';
import { ConfirmDialog } from '../components/common/ConfirmDialog';
import { timeAgo } from '../lib/format';
import { sdkReachability, type SdkReachability } from '../lib/sdkReachability';

/**
 * LagHound SDK-endpoint management. Register a customer endpoint (target URL +
 * write-only token + optional route), list existing endpoints (token shown
 * masked), and delete with confirmation. Mutations are operator-gated; viewers
 * get a read-only list. The Status column surfaces the latest sdkprobe run's
 * outcome (#765: dead endpoints rendered identically to live ones).
 */

const REACHABILITY_CHIP: Record<SdkReachability, { label: string; className: string }> = {
  reachable: { label: 'reachable', className: 'text-emerald-400 border-emerald-500/30 bg-emerald-500/10' },
  partial: { label: 'partial', className: 'text-yellow-400 border-yellow-500/30 bg-yellow-500/10' },
  unreachable: { label: 'unreachable', className: 'text-red-400 border-red-500/30 bg-red-500/10' },
  probing: { label: 'probing', className: 'text-cyan-400 border-cyan-500/30 bg-cyan-500/10' },
  none: { label: 'never probed', className: 'text-gray-500 border-gray-700 bg-transparent' },
};
export function SdkEndpointsPage() {
  const { projectId, isOperator } = useProject();
  const [endpoints, setEndpoints] = useState<SdkEndpoint[]>([]);
  const [samples, setSamples] = useState<SdkSampleStatus[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [createDefaults, setCreateDefaults] = useState<Partial<SdkEndpointCreate>>();
  const [showSamples, setShowSamples] = useState(false);
  const [sampleBusy, setSampleBusy] = useState<string | null>(null);
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
    // Sample state is a separate, best-effort read: it probes live hosts, so a
    // slow/dead sample must never block the endpoint list from rendering.
    api
      .getSdkSamples(projectId)
      .then((res) => setSamples(res.samples))
      .catch(() => setSamples([]));
  }, [projectId]);

  usePolling(load, 20000);

  /**
   * Register an already-serving sample as an SDK endpoint — the reuse path, and
   * the one that costs nothing. Deliberately routed through the same create
   * endpoint the dialog uses (reuse-first, idempotent), so "reuse one" and
   * "create with reuse" can never drift apart.
   */
  const reuseSample = async (language: string) => {
    if (!projectId) return;
    setSampleBusy(language);
    try {
      const res = await api.createSdkSamples(projectId, {
        shape: 'consolidated',
        languages: [language],
        reuse_existing: true,
      });
      addToast(
        'success',
        res.reused.length > 0
          ? `${language} sample registered as an SDK endpoint`
          : `${language} sample had nothing to reuse`,
      );
      load();
    } catch (e) {
      addToast('error', errorMessage(e));
    } finally {
      setSampleBusy(null);
    }
  };

  /** Re-run the sample's deployment in place so it picks up the current SDK. */
  const updateSample = async (language: string) => {
    if (!projectId) return;
    setSampleBusy(language);
    try {
      const res = await api.updateSdkSample(projectId, language);
      addToast(
        'success',
        `${language} sample updating${res.from_version ? ` from ${res.from_version}` : ''} to ${res.to_version}`,
      );
      load();
    } catch (e) {
      addToast('error', errorMessage(e));
    } finally {
      setSampleBusy(null);
    }
  };

  const openCreate = (defaults?: Partial<SdkEndpointCreate>) => {
    setCreateDefaults(defaults);
    setShowCreate(true);
  };

  const closeCreate = () => {
    setShowCreate(false);
    setCreateDefaults(undefined);
  };

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
      <PageShell title="SDK Endpoints" subtitle="Split request time into network and application work.">
        <div className="text-sm text-gray-400 motion-safe:animate-pulse">Loading registered endpoints…</div>
      </PageShell>
    );
  }

  if (error && endpoints.length === 0) {
    return (
      <PageShell title="SDK Endpoints" subtitle="Split request time into network and application work.">
        <div className="alert alert-error">
          <h3 className="text-red-400 font-bold mb-2">Failed to load SDK endpoints</h3>
          <p className="text-red-300 text-sm">{error}</p>
          <Button className="mt-4" onClick={load}>Retry</Button>
        </div>
      </PageShell>
    );
  }

  return (
    <PageShell
      title="SDK Endpoints"
      subtitle="Split request time into network and application work."
      action={
        isOperator ? (
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" onClick={() => setShowSamples(true)}>
              Deploy SDK samples
            </Button>
            <Button onClick={() => openCreate()}>
              + SDK endpoint
            </Button>
          </div>
        ) : undefined
      }
    >

      {showCreate && projectId && (
        <CreateSdkEndpointDialog
          projectId={projectId}
          initialValues={createDefaults}
          onClose={closeCreate}
          onCreated={load}
        />
      )}

      {showSamples && projectId && samples.length > 0 && (
        <CreateSdkSamplesDialog
          projectId={projectId}
          samples={samples}
          onClose={() => setShowSamples(false)}
          onCreated={load}
        />
      )}

      {projectId && samples.length > 0 && (
        <SdkSamplesPanel
          projectId={projectId}
          samples={samples}
          isOperator={isOperator}
          onDeploy={() => setShowSamples(true)}
          onReuse={(lang) => void reuseSample(lang)}
          onUpdate={(lang) => void updateSample(lang)}
          busy={sampleBusy}
        />
      )}

      {/* The public Azure reference apps: registering one of those costs
          nothing at all, so keep the shortcut — but only when a deployment URL
          is actually configured. Without one the panel was pure brochure (the
          "we can see the samples but we cannot create" complaint). */}
      {SDK_EXAMPLES.some((e) => e.liveUrl) && (
        <SdkExamplesPanel isOperator={isOperator} onUseExample={openCreate} />
      )}

      {error && endpoints.length > 0 && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh SDK endpoints. Retrying automatically.
        </div>
      )}

      <section className="section-divider" aria-labelledby="registered-sdk-endpoints">
        <div className="mb-3 flex flex-col gap-1 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <h2 id="registered-sdk-endpoints" className="text-sm font-bold text-gray-100">Registered endpoints</h2>
            <p className="mt-1 text-xs text-faint">
              {endpoints.length} endpoint{endpoints.length === 1 ? '' : 's'} in this project
            </p>
          </div>
          {endpoints.length > 0 && (
            <Link to={`/projects/${projectId}/reports/app-network`} className="text-xs text-cyan-400 hover:underline">
              Application Network Performance report →
            </Link>
          )}
        </div>

        {endpoints.length === 0 ? (
          <EmptyState
            message="No SDK endpoints yet"
            detail="Deploy the SDK samples above, or register a service of your own that mounts the LagHound SDK routes."
            action={
              isOperator ? (
                <div className="flex flex-wrap justify-center gap-2">
                  <Button variant="primary" onClick={() => setShowSamples(true)}>
                    Deploy SDK samples
                  </Button>
                  <Button onClick={() => openCreate()}>
                    Register your own service
                  </Button>
                </div>
              ) : undefined
            }
          />
        ) : (
          <DataTable
            columns={[
              {
                key: 'name',
                label: 'Name',
                cellClass: 'text-gray-200',
                render: (ep) => (
                  <>
                    {ep.name}
                    {ep.description && (
                      <div className="text-xs text-faint mt-0.5">{ep.description}</div>
                    )}
                  </>
                ),
              },
              {
                key: 'url',
                label: 'Target URL',
                cellClass: 'text-cyan-400 break-all',
                render: (ep) => ep.url ?? '—',
              },
              {
                key: 'route',
                label: 'Route',
                cellClass: 'text-gray-400',
                render: (ep) => ep.route ?? '/laghound/echo',
              },
              {
                key: 'token',
                label: 'Token',
                render: (ep) =>
                  ep.token_set ? (
                    <span className="text-gray-400" title="Token stored (write-only)">
                      {ep.token ?? '********'}
                    </span>
                  ) : (
                    <span className="text-yellow-500">not set</span>
                  ),
              },
              {
                key: 'status',
                label: 'Status',
                render: (ep) => {
                  const verdict = sdkReachability(ep);
                  const chip = REACHABILITY_CHIP[verdict];
                  return (
                    <>
                      <span
                        className={`inline-block border rounded px-1.5 py-0.5 text-xs ${chip.className}`}
                        title={
                          ep.last_run_status
                            ? `Last probe run: ${ep.last_run_status} (${ep.last_run_success_count ?? 0} ok / ${ep.last_run_failure_count ?? 0} failed)`
                            : 'No sdkprobe run has targeted this endpoint yet'
                        }
                      >
                        {chip.label}
                      </span>
                      {ep.last_run_at && (
                        <div className="text-xs text-faint mt-0.5">{timeAgo(ep.last_run_at)}</div>
                      )}
                    </>
                  );
                },
              },
              {
                key: 'created',
                label: 'Created',
                cellClass: 'text-gray-400',
                render: (ep) => new Date(ep.created_at).toLocaleString(),
              },
              ...(isOperator
                ? [
                    {
                      key: 'actions',
                      label: 'Actions',
                      align: 'right',
                      render: (ep) => (
                        <button
                          onClick={() => setConfirmDelete(ep)}
                          className="text-gray-400 hover:text-red-400 text-xs transition-colors"
                          aria-label={`Delete ${ep.name}`}
                        >
                          Delete
                        </button>
                      ),
                    } satisfies DataTableColumn<SdkEndpoint>,
                  ]
                : []),
            ]}
            rows={endpoints}
            rowKey={(ep) => ep.id}
          />
        )}
      </section>

      <ConfirmDialog
        open={Boolean(confirmDelete)}
        title="Delete SDK endpoint"
        description={confirmDelete ? `Delete ${confirmDelete.name}? This permanently removes the endpoint, its stored token, past probe runs, and report history.` : ''}
        confirmLabel="Delete"
        danger
        loading={deleting}
        onClose={() => setConfirmDelete(null)}
        onConfirm={() => void handleDelete()}
      />
    </PageShell>
  );
}
