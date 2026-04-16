import { useCallback, useEffect, useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { Deployment } from '../api/types';
import { testersApi, type TesterRow } from '../api/testers';
import { listVmHistory, type VmLifecycleRow } from '../api/vmHistory';
import { StatusBadge } from '../components/common/StatusBadge';
import { PageHeader } from '../components/common/PageHeader';
import { TesterRegionGroup } from '../components/TesterRegionGroup';
import { CreateTesterModal } from '../components/CreateTesterModal';
import { TesterDetailDrawer } from '../components/TesterDetailDrawer';
import { InfraDeployWizard } from '../components/InfraDeployWizard';
import type { DeployWizardPrefill } from '../components/DeployWizard';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useTesterSubscription } from '../hooks/useTesterSubscription';
import { useToast } from '../hooks/useToast';
import { formatDuration, timeAgo } from '../lib/format';

/* ── Tester grouping (from TestersPage) ── */

type GroupedTesters = { cloud: string; region: string; testers: TesterRow[] }[];

function groupByRegion(rows: TesterRow[]): GroupedTesters {
  const map = new Map<string, { cloud: string; region: string; testers: TesterRow[] }>();
  for (const row of rows) {
    const key = `${row.cloud}::${row.region}`;
    let entry = map.get(key);
    if (!entry) {
      entry = { cloud: row.cloud, region: row.region, testers: [] };
      map.set(key, entry);
    }
    entry.testers.push(row);
  }
  return Array.from(map.values()).sort((a, b) => {
    if (a.cloud !== b.cloud) return a.cloud.localeCompare(b.cloud);
    return a.region.localeCompare(b.region);
  });
}

/* ── History helpers ── */

const EVENT_BADGE: Record<string, string> = {
  created: 'text-cyan-400 border-cyan-500/30 bg-cyan-500/10',
  started: 'text-green-400 border-green-500/30 bg-green-500/10',
  stopped: 'text-gray-400 border-gray-500/30 bg-gray-500/10',
  auto_shutdown: 'text-amber-400 border-amber-500/30 bg-amber-500/10',
  deleted: 'text-red-400 border-red-500/30 bg-red-500/10',
  error: 'text-red-400 border-red-500/30 bg-red-500/10',
};

function formatTime(iso: string): string {
  return timeAgo(iso);
}

function EventBadge({ kind }: { kind: string }) {
  const cls =
    EVENT_BADGE[kind] ?? 'text-gray-400 border-gray-500/30 bg-gray-500/5';
  return (
    <span className={`inline-block text-[11px] px-2 py-0.5 rounded border font-mono ${cls}`}>
      {kind}
    </span>
  );
}

/* ── Create-modal defaults ── */

interface CreateDefaults {
  cloud: string;
  region: string;
  name: string;
  vmSize: string;
  autoShutdownEnabled: boolean;
  autoShutdownHour: number;
}

const FIRST_TESTER_DEFAULTS: CreateDefaults = {
  cloud: 'azure',
  region: 'eastus',
  name: 'eastus-1',
  vmSize: 'Standard_D2s_v3',
  autoShutdownEnabled: true,
  autoShutdownHour: 23,
};

/* ── Section divider ── */

function SectionDivider({
  title,
  count,
  action,
}: {
  title: string;
  count?: number;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between mb-3 mt-8 first:mt-0">
      <h3 className="text-xs text-gray-500 tracking-wider font-medium">
        {title}
        {count != null && count > 0 && (
          <span className="text-gray-600 ml-2">({count})</span>
        )}
      </h3>
      {action}
    </div>
  );
}

/* ══════════════════════════════════════════════════════════════════════
   InfrastructurePage — single scrollable page replacing tabbed layout
   ══════════════════════════════════════════════════════════════════════ */

export function InfrastructurePage() {
  usePageTitle('Infrastructure');
  const { projectId, isOperator, isProjectAdmin } = useProject();
  const addToast = useToast();
  const navigate = useNavigate();

  /* ── Targets state ── */
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [deploymentsLoading, setDeploymentsLoading] = useState(true);
  const [showWizard, setShowWizard] = useState(false);
  const [wizardKind, setWizardKind] = useState<'target' | 'runner'>('target');
  const [wizardPrefill, setWizardPrefill] = useState<DeployWizardPrefill | undefined>(undefined);

  /* ── Runners state ── */
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [testersLoading, setTestersLoading] = useState(true);
  const [selectedTester, setSelectedTester] = useState<TesterRow | null>(null);
  const [showCreateTester, setShowCreateTester] = useState(false);
  const [createDefaults, setCreateDefaults] = useState<CreateDefaults | null>(null);
  const [refreshingVersion, setRefreshingVersion] = useState(false);

  /* ── History state ── */
  const [history, setHistory] = useState<VmLifecycleRow[]>([]);
  const [historyLoading, setHistoryLoading] = useState(true);

  /* ── Errors ── */
  const [error, setError] = useState<string | null>(null);

  /* ── Data loading ── */
  const loadAll = useCallback(async () => {
    if (!projectId) return;
    try {
      const [deps, testerList, histResp] = await Promise.all([
        api.getDeployments(projectId, { limit: 50 }),
        testersApi.listTesters(projectId),
        listVmHistory(projectId, { limit: 10 }),
      ]);
      setDeployments(deps);
      setTesters(testerList);
      setHistory(histResp.events);
      setError(null);
    } catch {
      setError('Failed to load infrastructure data');
    } finally {
      setDeploymentsLoading(false);
      setTestersLoading(false);
      setHistoryLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  usePolling(() => void loadAll(), 10000, !!projectId);

  /* ── Open the wizard pre-filled to add stacks to an existing deployment.
        Pulls the cloud / region / OS / IP / installed proxies off the
        deployment's first endpoint so the user only ticks the new stacks. */
  const openAddStack = useCallback((d: Deployment) => {
    const ep = d.config?.endpoints?.[0];
    const ip = d.endpoint_ips?.[0] ?? '';
    if (!ep || !ip) return;
    const providerLower = (ep.provider || '').toLowerCase();
    const providerLabel: 'Azure' | 'AWS' | 'GCP' =
      providerLower === 'azure' ? 'Azure' :
      providerLower === 'aws' ? 'AWS' :
      providerLower === 'gcp' ? 'GCP' :
      'Azure';
    setWizardPrefill({
      cloud: providerLabel,
      cloudAccountId: '',
      region: ep.region ?? '',
      os: (ep.os === 'windows' ? 'windows' : 'linux'),
      existingVmIp: ip,
      installedProxies: ep.http_stacks ?? [],
    });
    setShowWizard(true);
  }, []);

  /* ── Tester grouping + WS subscription ── */
  const grouped = useMemo(() => groupByRegion(testers), [testers]);
  const testerIds = useMemo(() => testers.map((r) => r.tester_id), [testers]);
  const queueMap = useTesterSubscription(projectId, testerIds);

  /* ── Deployment buckets ── */
  const completedDeps = useMemo(() => deployments.filter(d => d.status === 'completed'), [deployments]);
  const activeDeps = useMemo(() => deployments.filter(d => d.status === 'running' || d.status === 'pending'), [deployments]);

  /* ── Tester callbacks ── */
  const handleAddTester = useCallback((cloud: string, region: string) => {
    setCreateDefaults({
      cloud,
      region,
      name: '',
      vmSize: 'Standard_D2s_v3',
      autoShutdownEnabled: true,
      autoShutdownHour: 23,
    });
    setShowCreateTester(true);
  }, []);

  const handleEmptyStateCreate = useCallback(() => {
    setCreateDefaults(FIRST_TESTER_DEFAULTS);
    setShowCreateTester(true);
  }, []);

  const handleRefreshVersion = useCallback(async () => {
    if (!projectId) return;
    setRefreshingVersion(true);
    try {
      const res = await testersApi.refreshLatestVersion(projectId);
      addToast('success', `Latest version: ${res.latest_version}`);
    } catch (e) {
      addToast(
        'error',
        e instanceof Error ? e.message : 'Failed to refresh version',
      );
    } finally {
      setRefreshingVersion(false);
    }
  }, [projectId, addToast]);

  return (
    <div className="p-4 md:p-6 max-w-6xl mx-auto">
      <PageHeader
        title="Infrastructure"
        subtitle="Targets and runners deployed across cloud regions"
      />

      {error && (
        <div role="alert" className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {/* ════════════════════════════════════════════════════════════════
          SECTION 1 — Targets (deployed test servers)
          ════════════════════════════════════════════════════════════════ */}

      {/* Active deploys in progress */}
      {activeDeps.length > 0 && (
        <div className="mb-4">
          <h3 className="text-xs text-gray-500 tracking-wider font-medium mb-3">in progress</h3>
          <div className="space-y-2">
            {activeDeps.map(d => (
              <Link
                key={d.deployment_id}
                to={`/projects/${projectId}/deploy/${d.deployment_id}`}
                className="block border border-blue-500/20 bg-blue-500/5 rounded p-3"
              >
                <div className="flex items-center justify-between mb-1">
                  <span className="text-cyan-400 text-sm">{d.name}</span>
                  <StatusBadge status={d.status} />
                </div>
                <div className="text-xs text-gray-500">{d.provider_summary || 'Deploying...'}</div>
              </Link>
            ))}
          </div>
        </div>
      )}

      <SectionDivider
        title="Targets"
        count={completedDeps.length}
        action={
          isOperator ? (
            <button
              type="button"
              onClick={() => { setWizardKind('target'); setWizardPrefill(undefined); setShowWizard(true); }}
              className="px-3 py-1 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white"
            >
              + Deploy
            </button>
          ) : null
        }
      />

      {deploymentsLoading && deployments.length === 0 ? (
        <div className="table-container">
          <div className="px-4 py-3 flex gap-8">
            {[80, 48, 56, 120, 48].map((w, i) => (
              <div key={i} className="h-3 rounded bg-gray-800/50 motion-safe:animate-pulse" style={{ width: w }} />
            ))}
          </div>
        </div>
      ) : completedDeps.length === 0 ? (
        <div className="border border-gray-800 rounded p-8 text-center">
          <p className="text-gray-500 text-sm">No targets deployed</p>
          {isOperator && (
            <button
              type="button"
              onClick={() => { setWizardKind('target'); setWizardPrefill(undefined); setShowWizard(true); }}
              className="text-xs text-cyan-400 mt-2"
            >
              Deploy your first target
            </button>
          )}
        </div>
      ) : (
        <>
          {/* Mobile */}
          <div className="md:hidden space-y-2">
            {completedDeps.map(d => (
              <Link
                key={d.deployment_id}
                to={`/projects/${projectId}/deploy/${d.deployment_id}`}
                className="block border border-gray-800 rounded p-3"
              >
                <div className="flex items-center justify-between mb-1">
                  <span className="text-cyan-400 text-sm">{d.name}</span>
                  <StatusBadge status={d.status} />
                </div>
                <div className="flex items-center gap-3 text-xs text-gray-500 flex-wrap">
                  <span>{d.provider_summary || '\u2014'}</span>
                  {d.endpoint_ips?.[0] && (
                    <span className="font-mono truncate max-w-[200px]">{d.endpoint_ips[0]}</span>
                  )}
                </div>
              </Link>
            ))}
          </div>

          {/* Desktop */}
          <div className="hidden md:block table-container">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="text-left px-4 py-2.5 font-medium">Name</th>
                  <th className="text-left px-4 py-2.5 font-medium">Provider</th>
                  <th className="text-left px-4 py-2.5 font-medium">Status</th>
                  <th className="text-left px-4 py-2.5 font-medium">Target</th>
                  <th className="text-left px-4 py-2.5 font-medium hidden lg:table-cell">Duration</th>
                  <th className="text-left px-4 py-2.5 font-medium hidden lg:table-cell">Created</th>
                  {isOperator && <th className="text-right px-4 py-2.5 font-medium" />}
                </tr>
              </thead>
              <tbody>
                {completedDeps.map(d => (
                  <tr key={d.deployment_id} className="border-b border-gray-800/30 hover:bg-gray-800/10">
                    <td className="px-4 py-3">
                      <Link to={`/projects/${projectId}/deploy/${d.deployment_id}`} className="text-cyan-400 hover:text-cyan-300">
                        {d.name}
                      </Link>
                    </td>
                    <td className="px-4 py-3 text-gray-400 text-xs">{d.provider_summary || '\u2014'}</td>
                    <td className="px-4 py-3"><StatusBadge status={d.status} /></td>
                    <td className="px-4 py-3 text-gray-400 font-mono text-xs truncate max-w-48">
                      {d.endpoint_ips?.[0] || '\u2014'}
                    </td>
                    <td className="px-4 py-3 text-gray-400 font-mono text-xs hidden lg:table-cell">
                      {formatDuration(d.started_at, d.finished_at)}
                    </td>
                    <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell" title={new Date(d.created_at).toISOString()}>
                      {timeAgo(d.created_at)}
                    </td>
                    {isOperator && (
                      <td className="px-4 py-3 text-right">
                        {d.status === 'running' && d.endpoint_ips?.[0] && (
                          <button
                            type="button"
                            onClick={() => openAddStack(d)}
                            className="text-[11px] px-2 py-1 rounded border border-gray-700 text-gray-400 hover:border-cyan-500/40 hover:text-cyan-300 transition-colors"
                            title="Install additional proxy stacks on this target"
                          >
                            + Add stack
                          </button>
                        )}
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}

      {/* ════════════════════════════════════════════════════════════════
          SECTION 2 — Runners (remote probe executors)
          ════════════════════════════════════════════════════════════════ */}

      <SectionDivider
        title="Runners"
        count={testers.length}
        action={
          isProjectAdmin ? (
            <div className="flex gap-2">
              <button
                type="button"
                disabled={refreshingVersion}
                onClick={handleRefreshVersion}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 disabled:opacity-50"
              >
                {refreshingVersion ? 'Refreshing\u2026' : 'Refresh latest version'}
              </button>
              <button
                type="button"
                onClick={() => { setWizardKind('runner'); setWizardPrefill(undefined); setShowWizard(true); }}
                className="px-3 py-1 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white"
              >
                + Deploy
              </button>
            </div>
          ) : null
        }
      />

      {testersLoading && testers.length === 0 ? (
        <p className="text-sm text-gray-500">Loading\u2026</p>
      ) : testers.length === 0 ? (
        <div
          className="border border-gray-800 rounded p-6 text-center"
          data-testid="testers-empty-state"
        >
          <h3 className="text-sm font-bold text-gray-200 mb-2">
            No runners yet
          </h3>
          <p className="text-xs text-gray-500 mb-4 max-w-md mx-auto">
            Create a persistent runner VM in your preferred region. It will be
            reused across benchmarks and stopped each night to save costs.
          </p>
          {isProjectAdmin && (
            <button
              type="button"
              onClick={handleEmptyStateCreate}
              className="px-4 py-1.5 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white"
            >
              + Create your first runner in eastus (recommended)
            </button>
          )}
        </div>
      ) : (
        grouped.map((g) => (
          <TesterRegionGroup
            key={`${g.cloud}-${g.region}`}
            cloud={g.cloud}
            region={g.region}
            testers={g.testers}
            queues={queueMap}
            onSelect={setSelectedTester}
            onAdd={handleAddTester}
          />
        ))
      )}

      {/* ════════════════════════════════════════════════════════════════
          SECTION 3 — Recent activity (last 10 VM lifecycle events)
          ════════════════════════════════════════════════════════════════ */}

      <SectionDivider
        title="Recent activity"
        action={
          <Link
            to={`/projects/${projectId}/vms/history`}
            className="text-xs text-gray-500 hover:text-cyan-400 transition-colors"
          >
            View full history &rarr;
          </Link>
        }
      />

      {historyLoading && history.length === 0 ? (
        <p className="text-sm text-gray-500">Loading\u2026</p>
      ) : history.length === 0 ? (
        <div className="border border-gray-800 rounded p-6 text-center">
          <p className="text-sm text-gray-400">No recent activity</p>
          <p className="text-xs text-gray-600 mt-1">VM lifecycle events will appear here</p>
        </div>
      ) : (
        <div className="table-container">
          <table className="w-full text-xs">
            <thead>
              <tr className="text-gray-500 border-b border-gray-800">
                <th className="px-3 py-2 text-left">When</th>
                <th className="px-3 py-2 text-left">Event</th>
                <th className="px-3 py-2 text-left">Resource</th>
                <th className="px-3 py-2 text-left hidden md:table-cell">Cloud / Region</th>
              </tr>
            </thead>
            <tbody>
              {history.map((r) => (
                <tr
                  key={r.event_id}
                  className="border-b border-gray-800/30 hover:bg-gray-800/20"
                >
                  <td className="px-3 py-1.5 text-gray-400 font-mono whitespace-nowrap" title={new Date(r.event_time).toISOString()}>
                    {formatTime(r.event_time)}
                  </td>
                  <td className="px-3 py-1.5">
                    <EventBadge kind={r.event_type} />
                  </td>
                  <td className="px-3 py-1.5 text-gray-200 font-mono truncate max-w-xs">
                    {r.resource_name ?? <span className="text-gray-600">(unnamed)</span>}
                  </td>
                  <td className="px-3 py-1.5 text-gray-400 font-mono hidden md:table-cell">
                    {r.cloud}
                    {r.region ? ` \u00b7 ${r.region}` : ''}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* ── Modals / Drawers ── */}

      {showWizard && (
        <InfraDeployWizard
          projectId={projectId}
          initialKind={wizardKind}
          prefillUpgrade={wizardPrefill && {
            cloud: wizardPrefill.cloud,
            cloudAccountId: wizardPrefill.cloudAccountId,
            region: wizardPrefill.region,
            os: wizardPrefill.os,
            existingVmIp: wizardPrefill.existingVmIp,
            installedProxies: wizardPrefill.installedProxies,
          }}
          onClose={() => { setShowWizard(false); setWizardPrefill(undefined); }}
          onCreated={(kind, id) => {
            setWizardPrefill(undefined);
            void loadAll();
            if (kind === 'target') {
              navigate(`/projects/${projectId}/deploy/${id}`);
            }
          }}
        />
      )}

      {showCreateTester && (
        <CreateTesterModal
          projectId={projectId}
          defaultCloud={createDefaults?.cloud}
          defaultRegion={createDefaults?.region}
          defaultName={createDefaults?.name}
          defaultVmSize={createDefaults?.vmSize}
          defaultAutoShutdownEnabled={createDefaults?.autoShutdownEnabled}
          defaultAutoShutdownHour={createDefaults?.autoShutdownHour}
          onCreated={() => {
            setShowCreateTester(false);
            setCreateDefaults(null);
            void loadAll();
          }}
          onClose={() => {
            setShowCreateTester(false);
            setCreateDefaults(null);
          }}
        />
      )}

      {selectedTester && (
        <TesterDetailDrawer
          projectId={projectId}
          tester={selectedTester}
          onClose={() => setSelectedTester(null)}
          onChanged={() => {
            void loadAll();
          }}
        />
      )}
    </div>
  );
}

export default InfrastructurePage;
