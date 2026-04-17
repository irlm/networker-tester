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

/**
 * Compact inline badge for the "recent activity" link row. Same color
 * palette as EventBadge but without the border/padding so it reads as a
 * tiny tag inside a one-line summary.
 */
function EventBadgeInline({ kind }: { kind: string }) {
  const cls =
    EVENT_BADGE[kind]?.split(' ').find(c => c.startsWith('text-'))
    ?? 'text-gray-400';
  return <span className={cls}>{kind}</span>;
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

/* ── Runner lifecycle/status derivation ──────────────────────────────── */

type Lifecycle = 'active' | 'archived' | 'all';
type RunnerStatus = 'idle' | 'busy' | 'stopped' | 'error';

/**
 * Map a raw tester row onto the four user-facing status buckets the filter
 * chips offer. Keeps the UI decoupled from backend power_state/allocation
 * names which have accumulated edge cases over time.
 */
function runnerStatus(row: TesterRow): RunnerStatus {
  if (row.power_state === 'error') return 'error';
  if (row.allocation === 'locked' || row.allocation === 'upgrading') return 'busy';
  if (row.power_state === 'stopped' || row.power_state === 'stopping') return 'stopped';
  // Provisioning/starting/upgrading VMs can't accept jobs yet — bucket them
  // as 'busy' so the header counts match the tests/new picker and users
  // don't launch against a runner that will never respond.
  if (row.power_state === 'starting' || row.power_state === 'provisioning' || row.power_state === 'upgrading') {
    return 'busy';
  }
  return 'idle';
}

/**
 * Archive semantics for runners aren't wired into the backend yet (see
 * project_infra_archive.md). Until then "Active" = every existing runner
 * and "Archived" stays empty with a helpful placeholder. The tab scaffold
 * lets us add the backend later without re-designing this page.
 */
const ARCHIVE_PLACEHOLDER_COUNT = 0;

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
  /* Lifecycle tab: Active / Archived / All. Per v1-refined filter UX, each
     tab owns its own chip row (status for Active, time scope for Archived,
     lifecycle+time for All). Backend archive is a follow-up; today Active
     == every runner, Archived == empty placeholder. */
  const [runnerTab, setRunnerTab] = useState<Lifecycle>('active');
  const [runnerStatusFilter, setRunnerStatusFilter] = useState<RunnerStatus | 'all'>('all');
  const [runnerPage, setRunnerPage] = useState(0);
  const RUNNER_PAGE_SIZE = 25;
  const [testersLoading, setTestersLoading] = useState(true);
  const [selectedTester, setSelectedTester] = useState<TesterRow | null>(null);
  const [showCreateTester, setShowCreateTester] = useState(false);
  const [createDefaults, setCreateDefaults] = useState<CreateDefaults | null>(null);
  const [refreshingVersion, setRefreshingVersion] = useState(false);

  /* ── History state — only the most-recent event shown inline; link
       drives the user to the full history page for scale. */
  const [history, setHistory] = useState<VmLifecycleRow[]>([]);
  const [, setHistoryLoading] = useState(true);

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

  /* ── Runner filter + pagination ──────────────────────────────────────
     Status chip filter runs against derived runnerStatus(row). Region
     grouping is preserved inside the tab so regional muscle memory still
     works. Pagination is client-side against the filtered list — with
     <200 runners this is plenty; virtualization can come later. */
  const runnerStatusCounts = useMemo(() => {
    const counts: Record<RunnerStatus | 'all', number> = {
      all: testers.length, idle: 0, busy: 0, stopped: 0, error: 0,
    };
    for (const t of testers) counts[runnerStatus(t)]++;
    return counts;
  }, [testers]);

  const filteredRunners = useMemo(() => {
    if (runnerTab === 'archived') return [] as TesterRow[]; // backend pending
    return runnerStatusFilter === 'all'
      ? testers
      : testers.filter(t => runnerStatus(t) === runnerStatusFilter);
  }, [testers, runnerTab, runnerStatusFilter]);

  const runnerTotalPages = Math.max(1, Math.ceil(filteredRunners.length / RUNNER_PAGE_SIZE));
  const runnerSafePage = Math.min(runnerPage, runnerTotalPages - 1);
  const pagedRunners = useMemo(
    () => filteredRunners.slice(runnerSafePage * RUNNER_PAGE_SIZE, (runnerSafePage + 1) * RUNNER_PAGE_SIZE),
    [filteredRunners, runnerSafePage],
  );

  /* ── Tester grouping + WS subscription ── */
  const grouped = useMemo(() => groupByRegion(pagedRunners), [pagedRunners]);
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

  const runnerIdleCt = runnerStatusCounts.idle;
  const runnerBusyCt = runnerStatusCounts.busy;
  const runnerStoppedCt = runnerStatusCounts.stopped;
  const runnerErrorCt = runnerStatusCounts.error;
  const runnerActiveCt = testers.length;
  const targetsCt = completedDeps.length;
  const cloudAccountsCt = 0; // wired later by the Settings cloud-accounts fetch; 0 is a reasonable default

  return (
    <div className="p-4 md:p-6 max-w-6xl mx-auto">
      <PageHeader
        title="Infrastructure"
        subtitle="Runners do the work; targets are what they probe."
      />

      {error && (
        <div role="alert" className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {/* ════════════════════════════════════════════════════════════════
          HERO — stats + primary deploy CTA
          ════════════════════════════════════════════════════════════════ */}
      <div className="grid grid-cols-1 md:grid-cols-[1fr_auto] gap-4 items-center p-4 md:p-5 border border-gray-800 bg-[var(--bg-raised)] mb-5">
        <div className="flex flex-wrap gap-6 md:gap-8">
          <div>
            <div className="text-2xl font-bold font-mono leading-none text-purple-300">{runnerActiveCt}</div>
            <div className="text-[10px] uppercase tracking-wider text-gray-500 mt-1.5">Runners active</div>
            <div className="text-[10px] text-green-400 font-mono mt-0.5">
              {runnerIdleCt} idle · {runnerBusyCt} busy · {runnerStoppedCt} stopped
              {runnerErrorCt > 0 && (
                <span className="text-red-400"> · {runnerErrorCt} error</span>
              )}
            </div>
          </div>
          <div>
            <div className="text-2xl font-bold font-mono leading-none text-cyan-300">{targetsCt}</div>
            <div className="text-[10px] uppercase tracking-wider text-gray-500 mt-1.5">Targets</div>
            {activeDeps.length > 0 && (
              <div className="text-[10px] text-amber-400 font-mono mt-0.5">
                {activeDeps.length} in progress
              </div>
            )}
          </div>
          <div>
            <div className="text-2xl font-bold font-mono leading-none text-gray-200">{cloudAccountsCt || '—'}</div>
            <div className="text-[10px] uppercase tracking-wider text-gray-500 mt-1.5">Cloud accounts</div>
          </div>
        </div>
        {isOperator && (
          <div className="flex flex-col gap-1.5 items-end">
            <button
              type="button"
              onClick={() => { setWizardKind('target'); setWizardPrefill(undefined); setShowWizard(true); }}
              className="px-5 py-2 text-sm font-medium bg-cyan-600 hover:bg-cyan-500 text-white"
            >
              + Deploy
            </button>
            <div className="flex gap-1.5">
              <button
                type="button"
                onClick={() => { setWizardKind('runner'); setWizardPrefill(undefined); setShowWizard(true); }}
                className="text-[10px] text-gray-500 hover:text-cyan-400 px-2 py-0.5 border border-gray-800 font-mono"
              >
                + runner
              </button>
              <button
                type="button"
                onClick={() => { setWizardKind('target'); setWizardPrefill(undefined); setShowWizard(true); }}
                className="text-[10px] text-gray-500 hover:text-cyan-400 px-2 py-0.5 border border-gray-800 font-mono"
              >
                + target
              </button>
            </div>
          </div>
        )}
      </div>

      {/* ════════════════════════════════════════════════════════════════
          SECTION 1 — Runners (moved above Targets per v1-refined)
          ════════════════════════════════════════════════════════════════ */}
      <SectionDivider
        title="Runners"
        count={runnerActiveCt}
        action={
          isProjectAdmin ? (
            <button
              type="button"
              disabled={refreshingVersion}
              onClick={handleRefreshVersion}
              className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 disabled:opacity-50"
            >
              {refreshingVersion ? 'Refreshing\u2026' : 'Refresh latest version'}
            </button>
          ) : null
        }
      />

      {/* Lifecycle tabs */}
      <div className="flex items-center gap-1 border-b border-gray-800 mb-3">
        {([
          { id: 'active' as const, icon: '●', label: 'Active', count: runnerActiveCt },
          { id: 'archived' as const, icon: '⊘', label: 'Archived', count: ARCHIVE_PLACEHOLDER_COUNT },
          { id: 'all' as const, icon: '≡', label: 'All time', count: runnerActiveCt + ARCHIVE_PLACEHOLDER_COUNT },
        ]).map(tab => {
          const active = runnerTab === tab.id;
          return (
            <button
              key={tab.id}
              type="button"
              onClick={() => { setRunnerTab(tab.id); setRunnerPage(0); }}
              className={`flex items-center gap-1.5 px-3 py-2 text-xs border-b-2 -mb-px transition-colors ${
                active
                  ? 'border-cyan-500 text-cyan-300'
                  : 'border-transparent text-gray-500 hover:text-gray-300'
              }`}
            >
              <span className="opacity-70">{tab.icon}</span>
              {tab.label}
              <span className={`ml-1 font-mono text-[10px] ${active ? 'text-cyan-400' : 'text-gray-600'}`}>
                {tab.count}
              </span>
            </button>
          );
        })}
      </div>

      {/* Per-tab filter chips */}
      {runnerTab === 'active' && (
        <div className="flex flex-wrap items-center gap-1.5 mb-3">
          <span className="text-[10px] uppercase tracking-wider text-gray-600 mr-2">status</span>
          {([
            { id: 'all' as const, label: 'All', ct: runnerStatusCounts.all },
            { id: 'idle' as const, label: 'Idle', ct: runnerStatusCounts.idle },
            { id: 'busy' as const, label: 'Busy', ct: runnerStatusCounts.busy },
            { id: 'stopped' as const, label: 'Stopped', ct: runnerStatusCounts.stopped },
            { id: 'error' as const, label: 'Error', ct: runnerStatusCounts.error },
          ]).map(c => {
            const active = runnerStatusFilter === c.id;
            return (
              <button
                key={c.id}
                type="button"
                onClick={() => { setRunnerStatusFilter(c.id); setRunnerPage(0); }}
                className={`px-2.5 py-1 text-xs border font-mono transition-colors ${
                  active
                    ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300'
                    : 'border-gray-700 text-gray-500 hover:text-gray-300'
                }`}
              >
                {c.label} <span className={active ? 'text-cyan-400' : 'text-gray-600'}>·{c.ct}</span>
              </button>
            );
          })}
        </div>
      )}

      {runnerTab === 'archived' && (
        <div className="flex flex-wrap items-center gap-1.5 mb-3">
          <span className="text-[10px] uppercase tracking-wider text-gray-600 mr-2">archived in</span>
          {['Last 7d', 'Last 30d', 'Last 90d', 'Last 12mo', 'All time'].map((label, i) => (
            <button
              key={label}
              type="button"
              disabled
              className={`px-2.5 py-1 text-xs border font-mono text-gray-600 border-gray-800 opacity-40 cursor-not-allowed ${
                i === 2 ? 'bg-cyan-500/5' : ''
              }`}
              title="Archive time-scope filter — lands with backend soft-delete"
            >
              {label} <span className="text-gray-700">·0</span>
            </button>
          ))}
        </div>
      )}

      {runnerTab === 'all' && (
        <div className="flex flex-wrap items-center gap-1.5 mb-3">
          <span className="text-[10px] uppercase tracking-wider text-gray-600 mr-2">lifecycle</span>
          {([
            { id: 'all', label: 'All', ct: runnerActiveCt + ARCHIVE_PLACEHOLDER_COUNT },
            { id: 'active-only', label: 'Active', ct: runnerActiveCt },
            { id: 'archived-only', label: 'Archived', ct: ARCHIVE_PLACEHOLDER_COUNT },
          ]).map(c => (
            <button
              key={c.id}
              type="button"
              disabled
              className="px-2.5 py-1 text-xs border font-mono text-gray-600 border-gray-800 opacity-40 cursor-not-allowed"
              title="Cross-lifecycle filter — lands with backend soft-delete"
            >
              {c.label} <span className="text-gray-700">·{c.ct}</span>
            </button>
          ))}
        </div>
      )}

      {/* Runners list — region-grouped inside the tab */}
      {testersLoading && testers.length === 0 ? (
        <p className="text-sm text-gray-500">Loading\u2026</p>
      ) : runnerTab === 'archived' ? (
        <div className="border border-dashed border-gray-800 rounded p-6 text-center">
          <p className="text-gray-500 text-sm">No archived runners</p>
          <p className="text-[11px] text-gray-600 font-mono mt-2">
            Archiving is a soft-delete — it preserves run-history attribution. Backend support lands next.
          </p>
        </div>
      ) : filteredRunners.length === 0 && testers.length > 0 ? (
        <div className="border border-gray-800 rounded p-6 text-center">
          <p className="text-gray-500 text-sm">No runners match the current filter</p>
          <button
            type="button"
            onClick={() => setRunnerStatusFilter('all')}
            className="text-xs text-cyan-400 mt-2"
          >
            Clear filter →
          </button>
        </div>
      ) : testers.length === 0 ? (
        <div
          className="border border-gray-800 rounded p-6 text-center"
          data-testid="testers-empty-state"
        >
          <h3 className="text-sm font-bold text-gray-200 mb-2">No runners yet</h3>
          <p className="text-xs text-gray-500 mb-4 max-w-md mx-auto">
            Create a persistent runner VM in your preferred region. It will be reused across benchmarks and stopped each night to save costs.
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

      {/* Pagination — only when a page overflow actually exists */}
      {filteredRunners.length > RUNNER_PAGE_SIZE && (
        <div className="flex items-center justify-between mt-3 text-xs text-gray-500">
          <span className="font-mono">
            showing {runnerSafePage * RUNNER_PAGE_SIZE + 1}–{Math.min((runnerSafePage + 1) * RUNNER_PAGE_SIZE, filteredRunners.length)} of {filteredRunners.length}
          </span>
          <div className="flex items-center gap-1">
            <button
              type="button"
              disabled={runnerSafePage === 0}
              onClick={() => setRunnerPage(p => Math.max(0, p - 1))}
              className="px-2 py-0.5 border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed font-mono"
            >
              ←
            </button>
            <span className="tabular-nums text-gray-400 px-2 font-mono">
              {runnerSafePage + 1} / {runnerTotalPages}
            </span>
            <button
              type="button"
              disabled={runnerSafePage >= runnerTotalPages - 1}
              onClick={() => setRunnerPage(p => Math.min(runnerTotalPages - 1, p + 1))}
              className="px-2 py-0.5 border border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600 disabled:opacity-30 disabled:cursor-not-allowed font-mono"
            >
              →
            </button>
          </div>
        </div>
      )}

      {/* ════════════════════════════════════════════════════════════════
          SECTION 2 — Targets (below runners per v1-refined)
          ════════════════════════════════════════════════════════════════ */}
      <SectionDivider
        title="Targets"
        count={completedDeps.length}
        action={
          activeDeps.length > 0 ? (
            <span className="text-[10px] text-amber-400 font-mono">
              {activeDeps.length} in progress
            </span>
          ) : null
        }
      />

      {/* Active deploys in progress banner */}
      {activeDeps.length > 0 && (
        <div className="space-y-2 mb-3">
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
      )}

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
                      <td className="px-4 py-3 text-right whitespace-nowrap">
                        {d.status === 'running' && d.endpoint_ips?.[0] && (
                          <>
                            <Link
                              to={`/projects/${projectId}/network/${d.deployment_id}`}
                              className="text-[11px] px-2 py-1 rounded border border-gray-700 text-gray-400 hover:border-cyan-500/40 hover:text-cyan-300 transition-colors mr-1.5"
                              title="See benchmark history for this endpoint"
                            >
                              ↗ Runs
                            </Link>
                            <button
                              type="button"
                              onClick={() => openAddStack(d)}
                              className="text-[11px] px-2 py-1 rounded border border-gray-700 text-gray-400 hover:border-cyan-500/40 hover:text-cyan-300 transition-colors"
                              title="Install additional proxy stacks on this target"
                            >
                              + Add stack
                            </button>
                          </>
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
          SECTION 3 — Recent activity (compact row; full history linked)
          ════════════════════════════════════════════════════════════════ */}

      <Link
        to={`/projects/${projectId}/vms/history`}
        className="mt-6 flex items-center justify-between px-3 py-2.5 border border-gray-800 bg-[var(--bg-surface)] hover:border-cyan-500/40 transition-colors"
      >
        <div className="flex items-center gap-3 text-xs text-gray-400 min-w-0">
          <span className="text-gray-600">≡</span>
          <span className="text-gray-300">Recent activity</span>
          {history.length > 0 ? (
            <span className="font-mono text-[11px] text-gray-500 truncate">
              {history[0].resource_name ?? '(unnamed)'} · <EventBadgeInline kind={history[0].event_type} /> · {formatTime(history[0].event_time)}
            </span>
          ) : (
            <span className="font-mono text-[11px] text-gray-600">no recent events</span>
          )}
        </div>
        <span className="text-xs text-cyan-400 flex-shrink-0">View full history →</span>
      </Link>

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
