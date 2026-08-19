import { useState, useEffect, useMemo, useCallback, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { timeAgo } from '../lib/format';
import { useAsyncEffect } from '../hooks/useAsyncEffect';
import { useNavigate } from 'react-router';
import { api } from '../api/client';
import { runsApi } from '../features/runs/api';
import { testersApi, type TesterRow } from '../api/testers';
import type { Deployment, TestRun, TestConfigCreate, Workload } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { familyOf, modeLabel } from '../components/common/mode-family';
import { RunResult } from '../components/common/RunResult';
import { unsupportedModes } from '../lib/mode-capabilities';
import { isOnlineTester } from '../lib/tester-readiness';
import { Button } from '../components/common/Button';
import { testConfigQueryOptions, useTestRunsQuery } from '../features/runs/queries';

// ── Mode families (source of truth is ModeChip.tsx) ────────────────────

interface ModeFamilyDef {
  id: 'net' | 'http' | 'thru' | 'page';
  label: string;
  modes: string[];
  activeClass: string;
  labelClass: string;
}

// Mode ids must match the backend Mode enum in
// crates/networker-common/src/test_config.rs (lowercase serde) — anything
// else is rejected with a 422 when the config is created.
// One selection accent across all families (audit F12 — the four category
// hues collided with the status ramp; grouping carries the taxonomy now).
const CHIP_ACTIVE_CLASS = 'bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50';

function stepMarkerClass(complete: boolean): string {
  if (complete) return 'w-5 h-5 rounded-full text-xs text-center leading-[18px] border bg-cyan-500 text-black border-cyan-500';
  return 'w-5 h-5 rounded-full text-xs text-center leading-[18px] border bg-gray-900 text-gray-400 border-gray-700';
}

function runnerChoiceClass(active: boolean): string {
  if (active) return 'px-2.5 py-1 text-xs border border-cyan-500/40 text-cyan-300 bg-cyan-500/5 transition-colors';
  return 'px-2.5 py-1 text-xs border border-gray-800 text-gray-400 transition-colors';
}

const MODE_FAMILIES: ModeFamilyDef[] = [
  {
    id: 'net',
    label: 'NETWORK',
    // ping/path/dualstack/pmtud are any-target; rpm/websocket need exactly
    // this page's target (a deployed networker-endpoint). All family `net`
    // in shared/modes.json.
    modes: ['tcp', 'dns', 'tls', 'tlsresume', 'native', 'udp', 'ping', 'path', 'dualstack', 'pmtud', 'rpm', 'websocket'],
    activeClass: CHIP_ACTIVE_CLASS,
    labelClass: 'text-gray-400',
  },
  {
    id: 'http',
    label: 'HTTP',
    modes: ['http1', 'http2', 'http3', 'curl'],
    activeClass: CHIP_ACTIVE_CLASS,
    labelClass: 'text-gray-400',
  },
  {
    id: 'thru',
    label: 'THROUGHPUT',
    modes: ['download', 'upload'],
    activeClass: CHIP_ACTIVE_CLASS,
    labelClass: 'text-gray-400',
  },
  {
    id: 'page',
    label: 'PAGE-LOAD',
    modes: ['pageload', 'pageload2', 'pageload3'],
    activeClass: CHIP_ACTIVE_CLASS,
    labelClass: 'text-gray-400',
  },
];

const FAMILY_BY_ID = new Map(MODE_FAMILIES.map(f => [f.id, f]));

// Modes that measure throughput — they need explicit payload sizes, otherwise
// the agent gets an empty list and the run completes with zero data moved.
const THROUGHPUT_MODES = new Set(['download', 'upload']);

const PAYLOAD_PRESETS: Array<{ bytes: number; label: string }> = [
  { bytes: 1024, label: '1 KB' },
  { bytes: 64 * 1024, label: '64 KB' },
  { bytes: 1024 * 1024, label: '1 MB' },
  { bytes: 10 * 1024 * 1024, label: '10 MB' },
  { bytes: 100 * 1024 * 1024, label: '100 MB' },
];
const DEFAULT_PAYLOADS = [1024 * 1024]; // 1 MB — sensible single-size default.

const MODE_PRESETS: Array<{ id: string; label: string; modes: string[]; desc: string }> = [
  { id: 'quick',    label: '★ Quick check',    modes: ['tcp','dns','tls','http1','http2','http3'], desc: 'net + http' },
  { id: 'http',     label: '★ HTTP versions',  modes: ['http1','http2','http3'],                    desc: 'h1/h2/h3' },
  { id: 'thruput',  label: '★ Throughput',     modes: ['download','upload'],                        desc: 'download + upload' },
  { id: 'full',     label: '★ Full sweep',     modes: MODE_FAMILIES.flatMap(f => f.modes),          desc: 'everything' },
];

const ALL_MODES = new Set(MODE_FAMILIES.flatMap(f => f.modes));

/** Region of a deployment's first endpoint. Wizard-authored deploy.json puts
 *  `region` at the endpoint level, but auto-provisioned configs
 *  (BuildDeployJson) nest it under the provider block
 *  (`endpoints[0].azure.region`) — reading only the top level rendered every
 *  auto-provisioned card as "region unknown" (E2E P2-9). */
function deploymentRegion(d: Deployment): string | undefined {
  const ep = d.config?.endpoints?.[0];
  if (!ep) return undefined;
  if (ep.region) return ep.region;
  const block = (ep as unknown as Record<string, { region?: string } | undefined>)[ep.provider];
  return block?.region;
}

function classForMode(mode: string, active: boolean): string {
  if (!active) return 'border-gray-700 text-gray-400 hover:text-gray-300 hover:border-gray-600';
  // Prefer the page-local family table (covers 'native', which the shared
  // familyOf() map doesn't know), fall back to familyOf for legacy modes
  // that only appear on historical runs.
  const family = MODE_FAMILIES.find(f => f.modes.includes(mode))?.id ?? familyOf(mode);
  return FAMILY_BY_ID.get(family as ModeFamilyDef['id'])?.activeClass
    ?? 'bg-gray-700/30 text-gray-400 border-gray-700';
}


function deploymentStatusDot(status: string): string {
  switch (status) {
    case 'running': return 'bg-green-400';
    case 'stopped': case 'stopping': return 'bg-gray-500';
    case 'error': case 'failed':     return 'bg-red-400';
    case 'creating': case 'starting': return 'bg-yellow-400';
    default: return 'bg-gray-600';
  }
}

// ── Component ──────────────────────────────────────────────────────────

export function NetworkTestPage() {
  const { projectId } = useProject();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Network Test');

  // Data
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [resourcesLoading, setResourcesLoading] = useState(true);
  const recentRunsQuery = useTestRunsQuery(
    projectId,
    { endpoint_kind: 'network', limit: 5 },
    { polling: false },
  );
  const recentRuns = useMemo(() => recentRunsQuery.data ?? [], [recentRunsQuery.data]);
  const loading = resourcesLoading || recentRunsQuery.isPending;

  // Form state — intent-first: modes → target → runner.
  // ?modes= / ?target= seed the INITIAL values rather than being written back
  // by an effect: the effect version rendered empty defaults first and then
  // replaced them, a visible flicker, and set state during the effect body.
  const urlParams = new URLSearchParams(window.location.search);
  const urlModes = (urlParams.get('modes') ?? '')
    .split(',').map(m => m.trim()).filter(m => ALL_MODES.has(m));
  // `rawSelectedModes` is the user's intent; `selectedModes` (below) is the
  // DERIVED subset the chosen target/runner can actually run, so switching
  // to an apache target silently un-checks http3 and switching back restores it.
  const [rawSelectedModes, setSelectedModes] = useState<Set<string>>(() => new Set(urlModes));
  const [activePreset, setActivePreset] = useState<string | null>(null);
  const [payloadSizes, setPayloadSizes] = useState<Set<number>>(new Set(DEFAULT_PAYLOADS));
  const [selectedTargetId, setSelectedTargetId] = useState<string>(
    () => urlParams.get('target') ?? '',
  );
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);

  // Per-target capability filter ("the target must return the tests
  // supported"): modes the SELECTED target's live self-report marks
  // unsupported (mode → reason). Stored keyed by target id and DERIVED back
  // to a map, so switching targets clears it without a synchronous
  // setState-in-effect; only ever narrows on POSITIVE knowledge — no report
  // (unreachable / pre-0.28.202) means no filtering.
  const [targetCaps, setTargetCaps] = useState<{ targetId: string; off: Map<string, string> } | null>(null);
  const liveUnsupported = useMemo(
    () => (targetCaps?.targetId === selectedTargetId ? targetCaps.off : new Map<string, string>()),
    [targetCaps, selectedTargetId],
  );
  // The full gate (lib/mode-capabilities): the manifest `requires` rule for an
  // endpoint target, the HTTP/3-by-stack rule for the deployment's proxy stack
  // (shared/http-stacks.json — apache / haproxy / traefik have no QUIC; the
  // config-create API rejects those with 422 too), the live self-report above,
  // and the pinned runner's Chrome for the browser modes.
  const selectedDeploymentForCaps = useMemo(
    () => deployments.find(d => d.deployment_id === selectedTargetId) ?? null,
    [deployments, selectedTargetId],
  );
  const pinnedRunner = useMemo(
    () => (runnerMode === 'specific' && selectedTesterId
      ? testers.find(t => t.tester_id === selectedTesterId)?.agent_capabilities ?? null
      : null),
    [runnerMode, selectedTesterId, testers],
  );
  const targetUnsupported = useMemo(
    () => unsupportedModes(ALL_MODES, {
      kind: 'endpoint',
      // http_stacks[0] is the listener a `proxy` config resolves to (the
      // dispatcher's rule); a deployment without stacks is the bare endpoint.
      stack: selectedDeploymentForCaps?.config?.endpoints?.[0]?.http_stacks?.[0] ?? null,
      unsupported: liveUnsupported,
      runner: pinnedRunner,
    }),
    [selectedDeploymentForCaps, liveUnsupported, pinnedRunner],
  );
  const selectedModes = useMemo(
    () => new Set([...rawSelectedModes].filter(m => !targetUnsupported.has(m))),
    [rawSelectedModes, targetUnsupported],
  );
  useEffect(() => {
    if (!selectedTargetId) return;
    let cancelled = false;
    api.getDeploymentCapabilities(projectId, selectedTargetId)
      .then(c => {
        if (cancelled) return;
        const reporting = c.endpoints.filter(e => e.supported_modes != null);
        // A mode is off only when EVERY reporting host says so — a
        // multi-endpoint deployment supports what any of its hosts serves.
        const off = new Map<string, string>();
        for (const u of reporting[0]?.unsupported_modes ?? []) {
          if (reporting.every(r => r.unsupported_modes?.some(x => x.mode === u.mode))) {
            off.set(u.mode, u.reason);
          }
        }
        // No pruning of the user's picks here: `selectedModes` is DERIVED from
        // rawSelectedModes minus targetUnsupported, so the launch can never
        // include an off mode and switching target restores the intent.
        setTargetCaps({ targetId: selectedTargetId, off });
      })
      .catch(() => { /* no report — no filtering */ });
    return () => { cancelled = true; };
  }, [projectId, selectedTargetId]);
  const [targetSearch, setTargetSearch] = useState('');
  const [targetPopoverOpen, setTargetPopoverOpen] = useState(false);
  const [runnerExpanded, setRunnerExpanded] = useState(false);

  // Scope tab for recent runs. "Mine only" is intentionally absent — the
  // backend doesn't expose user_id on TestRun yet, so it can't be honest.
  const [scopeTab, setScopeTab] = useState<'all' | 'this'>('all');

  const [submitting, setSubmitting] = useState(false);
  const targetInputRef = useRef<HTMLInputElement>(null);
  const formRef = useRef<HTMLDivElement>(null);

  // Prefill from query params (?modes=a,b&target=<deploymentId>) — set by
  // the EndpointRunsPage preset cards and its per-run rerun links.


  // ── Data loading ─────────────────────────────────────────────────────

  // `loading` starts true, so the removed synchronous setLoading(true) only
  // mattered on a projectId change — where the list now stays visible until the
  // new data lands instead of flashing a spinner. useAsyncEffect owns the
  // cancellation flag this effect used to hand-roll.
  useAsyncEffect((cancelled) => Promise.all([
      api.getDeployments(projectId, { limit: 50 }).catch(() => [] as Deployment[]),
      testersApi.listTesters(projectId).catch(() => [] as TesterRow[]),
    ]).then(([deps, rnrs]) => {
      if (cancelled()) return;
      // Only COMPLETED deployments are runnable targets — failed/cancelled ones
      // have no live endpoint and used to be listed (and selectable!) here,
      // producing guaranteed-failing runs (E2E P2-9).
      setDeployments(deps.filter(d => d.status === 'completed'));
      setTesters(rnrs);
      setResourcesLoading(false);
    }), [projectId]);

  // ── Derived ──────────────────────────────────────────────────────────

  const lastRun = recentRuns[0] ?? null;
  const selectedDeployment = selectedDeploymentForCaps;
  const runnerStats = useMemo(() => {
    // "online" requires a CONNECTED agent, not just a powered-on VM —
    // matches the Infrastructure page and the dashboard KPI (2026-08 UI
    // pass). A running VM whose agent is dark can't take this job.
    const online = testers.filter(isOnlineTester);
    const idle = online.filter(t => t.allocation === 'idle');
    return { online: online.length, idle: idle.length };
  }, [testers]);

  const filteredTargets = useMemo(() => {
    const q = targetSearch.trim().toLowerCase();
    if (!q) return deployments;
    return deployments.filter(d =>
      d.name.toLowerCase().includes(q)
      || d.config?.endpoints?.[0]?.provider?.toLowerCase().includes(q)
      || (deploymentRegion(d) ?? '').toLowerCase().includes(q)
      // The hostname/IP is what an engineer actually knows about a target —
      // it wasn't searchable (E2E P2-9).
      || (d.endpoint_ips ?? []).some(h => h.toLowerCase().includes(q))
      || (d.endpoint_hosts ?? []).some(h => (h ?? '').toLowerCase().includes(q)),
    );
  }, [deployments, targetSearch]);

  const filteredRecent = useMemo(() => {
    if (scopeTab === 'this' && selectedDeployment) {
      // TestRun doesn't expose target id directly; filter by config_name contains target name as a best-effort
      return recentRuns.filter(r => r.config_name?.toLowerCase().includes(selectedDeployment.name.toLowerCase()));
    }
    return recentRuns;
  }, [recentRuns, scopeTab, selectedDeployment]);

  const needsPayload = useMemo(
    () => [...selectedModes].some(m => THROUGHPUT_MODES.has(m)),
    [selectedModes]
  );
  const canLaunch = selectedModes.size > 0 && selectedTargetId !== ''
    && (!needsPayload || payloadSizes.size > 0);

  // ── Mode helpers ─────────────────────────────────────────────────────

  const toggleMode = useCallback((mode: string) => {
    setActivePreset(null);
    setSelectedModes(prev => {
      const next = new Set(prev);
      if (next.has(mode)) next.delete(mode); else next.add(mode);
      return next;
    });
  }, []);

  const toggleFamily = useCallback((family: ModeFamilyDef) => {
    setActivePreset(null);
    setSelectedModes(prev => {
      const next = new Set(prev);
      const eligible = family.modes.filter(m => !targetUnsupported.has(m));
      const allSelected = eligible.every(m => next.has(m));
      if (allSelected) eligible.forEach(m => next.delete(m));
      else eligible.forEach(m => next.add(m));
      return next;
    });
  }, [targetUnsupported]);

  const applyPreset = useCallback((presetId: string) => {
    const preset = MODE_PRESETS.find(p => p.id === presetId);
    if (!preset) return;
    setActivePreset(presetId);
    setSelectedModes(new Set(preset.modes.filter(m => !targetUnsupported.has(m))));
  }, [targetUnsupported]);

  const clearModes = useCallback(() => {
    setActivePreset(null);
    setSelectedModes(new Set());
  }, []);

  // ── Rerun ────────────────────────────────────────────────────────────

  const rerunConfig = useCallback(async (configId: string) => {
    if (submitting) return;
    setSubmitting(true);
    try {
      const run = await runsApi.launchConfig(configId);
      addToast('success', `Run ${run.id.slice(0, 8)} launched`);
      navigate(`/projects/${projectId}/runs/${run.id}`);
    } catch (e) {
      addToast('error', `Failed to launch: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  }, [submitting, addToast, navigate, projectId]);

  const tweakFromRun = useCallback(async (run: TestRun) => {
    try {
      const cfg = await queryClient.ensureQueryData(testConfigQueryOptions(run.test_config_id));
      // Prefill modes
      setSelectedModes(new Set(cfg.workload.modes));
      setActivePreset(null);
      // Prefill target (best-effort: match by proxy_endpoint_id on 'proxy' endpoints)
      if (cfg.endpoint.kind === 'proxy' && 'proxy_endpoint_id' in cfg.endpoint) {
        setSelectedTargetId(cfg.endpoint.proxy_endpoint_id);
      }
      setRunnerMode(run.tester_id ? 'specific' : 'auto');
      setSelectedTesterId(run.tester_id);
      // Scroll to form
      formRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      addToast('info', `Loaded config from ${run.id.slice(0, 8)} — tweak and launch.`);
    } catch (e) {
      addToast('error', `Failed to load config: ${e instanceof Error ? e.message : String(e)}`);
    }
  }, [addToast, queryClient]);

  // ── Launch new ───────────────────────────────────────────────────────

  const launchNew = useCallback(async () => {
    if (!canLaunch || submitting || !selectedDeployment) return;
    setSubmitting(true);
    try {
      const needsPayload = [...selectedModes].some(m => THROUGHPUT_MODES.has(m));
      if (needsPayload && payloadSizes.size === 0) {
        addToast('error', 'Pick at least one payload size for throughput modes.');
        setSubmitting(false);
        return;
      }
      const workload: Workload = {
        modes: [...selectedModes],
        runs: 10,
        concurrency: 1,
        timeout_ms: 5000,
        payload_sizes: needsPayload ? [...payloadSizes].sort((a, b) => a - b) : [],
        capture_mode: 'headers-only',
      };
      const name = `${selectedDeployment.name}-${[...selectedModes].slice(0, 3).join('-')}-${Date.now().toString(36).slice(-4)}`;
      const config: TestConfigCreate = {
        name,
        test_kind: 'network',
        endpoint: { kind: 'proxy', proxy_endpoint_id: selectedDeployment.deployment_id },
        workload,
      };
      const created = await runsApi.createConfig(projectId, config);
      const run = await runsApi.launchConfig(created.id, selectedTesterId ?? undefined);
      addToast('success', `Run ${run.id.slice(0, 8)} launched`);
      navigate(`/projects/${projectId}/runs/${run.id}`);
    } catch (e) {
      addToast('error', `Failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  }, [canLaunch, submitting, selectedDeployment, selectedModes, selectedTesterId, payloadSizes, projectId, addToast, navigate]);

  // ── Keyboard shortcuts ───────────────────────────────────────────────

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Ignore when typing in an input
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) {
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if ((e.key === 'r' || e.key === 'R') && lastRun) {
        e.preventDefault();
        rerunConfig(lastRun.test_config_id);
      } else if (/^[1-5]$/.test(e.key)) {
        const idx = Number(e.key) - 1;
        const run = filteredRecent[idx];
        if (run) {
          e.preventDefault();
          rerunConfig(run.test_config_id);
        }
      } else if (e.key === '/') {
        e.preventDefault();
        targetInputRef.current?.focus();
      } else if (e.key === 'Enter' && canLaunch) {
        e.preventDefault();
        launchNew();
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [lastRun, filteredRecent, canLaunch, rerunConfig, launchNew]);

  // ── Render ───────────────────────────────────────────────────────────

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <Breadcrumb items={[{ label: 'Network', to: `/projects/${projectId}/runs` }, { label: 'New Test' }]} />

      <div className="mb-4">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">New Network Test</h2>
        <p className="text-xs text-gray-400 mt-1">
          Run network primitives against a deployed target. Most runs repeat — rerun a recent run below, or build a new one.
        </p>
      </div>

      {/* ─── HERO: Rerun last ──────────────────────────────────────── */}
      {lastRun && lastRun.modes && lastRun.modes.length > 0 && (
        <div className="grid grid-cols-[auto_1fr_auto_auto] items-center gap-3 mb-3 px-5 py-3 border border-cyan-500/40 bg-cyan-500/5">
          <div className="text-cyan-400 text-xl leading-none">↻</div>
          <div>
            <div className="text-xs uppercase tracking-wider text-gray-400">Repeat last run</div>
            <div className="text-sm text-gray-200 mt-0.5">
              <span className="text-cyan-300">{lastRun.config_name ?? lastRun.id.slice(0, 8)}</span>
            </div>
            <div className="flex items-center gap-1 flex-wrap mt-1 text-xs text-gray-400">
              {lastRun.modes.slice(0, 8).map(m => (
                <span
                  key={m}
                  className={`inline-flex items-center px-1.5 py-0.5 text-xs leading-tight border rounded-sm ${classForMode(m, true)}`}
                >
                  {modeLabel(m)}
                </span>
              ))}
              {lastRun.modes.length > 8 && (
                <span className="text-xs text-faint">+{lastRun.modes.length - 8}</span>
              )}
              <span className="ml-2">· {timeAgo(lastRun.finished_at ?? lastRun.started_at ?? lastRun.created_at)}</span>
              {lastRun.status === 'completed' && (
                <span className="ml-2">· <RunResult ok={lastRun.success_count} fail={lastRun.failure_count} /></span>
              )}
              <span className="ml-2">
                · <button onClick={() => navigate(`/projects/${projectId}/runs/${lastRun.id}`)} className="text-cyan-400 hover:underline">view last run</button>
              </span>
            </div>
          </div>
          <button
            onClick={() => tweakFromRun(lastRun)}
            className="px-2.5 py-1.5 text-xs border border-gray-700 text-gray-400 hover:border-cyan-500/40 hover:text-cyan-300 transition-colors"
            title="Load this config into the form below"
          >
            ✎ tweak &amp; run
          </button>
          <Button
            variant="primary"
            onClick={() => rerunConfig(lastRun.test_config_id)}
            loading={submitting}
            loadingLabel="Running…"
            className="flex items-center gap-2"
          >
            ▶ Run again
            <span className="px-1.5 py-0.5 text-xs bg-black/25 border border-white/20 rounded">R</span>
          </Button>
        </div>
      )}

      {/* ─── Recent runs ─────────────────────────────────────────── */}
      {recentRuns.length > 1 && (
        <div className="mb-4">
          <div className="flex items-baseline justify-between mb-1.5">
            <h3 className="text-xs font-semibold text-gray-300 tracking-wider">Recent runs</h3>
            <span className="text-xs text-faint">rerun any row or click → for detail</span>
          </div>
          <div className="flex items-center gap-1 mb-2 text-xs">
            {([
              { id: 'all' as const, label: 'All', count: recentRuns.length },
              { id: 'this' as const, label: 'This target only', count: selectedDeployment ? filteredRecent.length : 0 },
            ]).map(t => (
              <button
                key={t.id}
                onClick={() => setScopeTab(t.id)}
                disabled={t.id === 'this' && !selectedDeployment}
                className={`px-2.5 py-0.5 border transition-colors ${
                  scopeTab === t.id
                    ? 'border-cyan-500/40 text-cyan-300 bg-cyan-500/5'
                    : 'border-transparent text-gray-400 hover:text-gray-300'
                } disabled:opacity-40 disabled:cursor-not-allowed`}
              >
                {t.label} <span className="text-xs text-faint">· {t.count}</span>
              </button>
            ))}
            <div className="flex-1" />
            <button onClick={() => navigate(`/projects/${projectId}/runs`)} className="text-cyan-400 hover:underline">view all →</button>
          </div>

          <div className="border-t border-gray-800/50">
            {filteredRecent.slice(0, 5).map((run, i) => {
              const modes = run.modes ?? [];
              return (
                <div
                  key={run.id}
                  className="grid items-center gap-3 py-2 px-3 border-b border-gray-800/50 hover:bg-cyan-500/[.03]"
                  style={{ gridTemplateColumns: '28px 1fr auto auto 100px' }}
                >
                  <span className="inline-block px-1.5 py-0.5 text-xs text-gray-400 bg-gray-900 border border-gray-800 rounded text-center">
                    {i + 1}
                  </span>
                  <div className="min-w-0">
                    <span className="text-xs text-gray-200">{run.config_name ?? run.id.slice(0, 8)}</span>
                    <span className="text-xs text-gray-400 ml-2">{timeAgo(run.finished_at ?? run.started_at ?? run.created_at)}</span>
                  </div>
                  <div className="flex gap-1 flex-wrap">
                    {modes.slice(0, 6).map(m => (
                      <span
                        key={m}
                        className={`inline-flex items-center px-1.5 py-0.5 text-xs leading-tight border rounded-sm ${classForMode(m, true)}`}
                      >
                        {modeLabel(m)}
                      </span>
                    ))}
                    {modes.length > 6 && <span className="text-xs text-faint">+{modes.length - 6}</span>}
                  </div>
                  <RunResult ok={run.success_count} fail={run.failure_count} className="text-xs" />
                  <div className="text-right text-xs">
                    <button onClick={() => rerunConfig(run.test_config_id)} disabled={submitting} className="text-cyan-400 hover:underline disabled:opacity-50" title={`rerun (key ${i + 1})`}>↻ rerun</button>
                    <button onClick={() => tweakFromRun(run)} className="text-gray-400 hover:text-cyan-300 ml-2" title="load into form">✎</button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* ─── OR DIVIDER ──────────────────────────────────────────── */}
      {recentRuns.length > 0 && (
        <div className="flex items-center gap-3 my-6 text-xs uppercase tracking-widest text-faint">
          <div className="flex-1 h-px bg-gray-800" />
          or build a new run
          <div className="flex-1 h-px bg-gray-800" />
        </div>
      )}

      {/* ─── NEW-RUN FORM ────────────────────────────────────────── */}
      <div ref={formRef} className="border border-gray-800 p-5">

        {/* Step 1: MODES */}
        <div className="mb-5">
          <div className="flex items-baseline gap-2 mb-2">
            <span className={stepMarkerClass(selectedModes.size > 0)}>1</span>
            <span className="text-xs text-gray-200 font-medium">What are you testing?</span>
            <span className="text-xs text-gray-400 ml-auto">
              {selectedModes.size === 0 ? 'pick at least one mode' : `${selectedModes.size} mode${selectedModes.size === 1 ? '' : 's'} selected`}
            </span>
          </div>

          {/* Preset chips */}
          <div className="flex gap-1.5 flex-wrap mb-3">
            {MODE_PRESETS.map(p => {
              const active = activePreset === p.id;
              return (
                <button
                  key={p.id}
                  onClick={() => applyPreset(p.id)}
                  className={`px-2.5 py-1 text-xs border transition-colors ${
                    active
                      ? 'border-cyan-500 text-cyan-300 bg-cyan-500/10'
                      : 'border-gray-800 border-dashed text-gray-400 hover:text-cyan-300 hover:border-cyan-500/40 hover:border-solid'
                  }`}
                  title={p.desc}
                >
                  {p.label}
                </button>
              );
            })}
            {selectedModes.size > 0 && (
              <button onClick={clearModes} className="px-2.5 py-1 text-xs text-faint hover:text-gray-300">clear all</button>
            )}
          </div>

          {/* Payload sizes — only shown when any throughput mode is active, since
              other modes ignore payload. Keeps the form compact by default. */}
          {[...selectedModes].some(m => THROUGHPUT_MODES.has(m)) && (
            <div className="mb-2 px-2 py-1.5 border border-cyan-400/30 bg-cyan-500/5 rounded-sm">
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs uppercase tracking-wider text-gray-400">
                  PAYLOAD SIZES <span className="text-gray-400 normal-case tracking-normal">· download/upload run once per selected size</span>
                </span>
                <span className="text-xs text-gray-400">
                  {payloadSizes.size === 0 ? 'pick at least one' : `${payloadSizes.size} selected`}
                </span>
              </div>
              <div className="flex gap-1 flex-wrap">
                {PAYLOAD_PRESETS.map(p => {
                  const active = payloadSizes.has(p.bytes);
                  return (
                    <button
                      key={p.bytes}
                      onClick={() => setPayloadSizes(prev => {
                        const next = new Set(prev);
                        if (next.has(p.bytes)) next.delete(p.bytes); else next.add(p.bytes);
                        return next;
                      })}
                      className={`px-2 py-0.5 text-xs border transition-colors rounded-sm ${
                        active
                          ? CHIP_ACTIVE_CLASS
                          : 'border-gray-700 text-gray-400 hover:text-gray-300 hover:border-gray-600'
                      }`}
                    >
                      {p.label}
                    </button>
                  );
                })}
              </div>
            </div>
          )}

          {/* Mode families */}
          {MODE_FAMILIES.map(family => {
            // "select all" counts only what THIS target/runner can run.
            const eligible = family.modes.filter(m => !targetUnsupported.has(m));
            const allSelected = eligible.length > 0 && eligible.every(m => selectedModes.has(m));
            return (
              <div key={family.id} className="mb-2">
                <div className="flex items-center justify-between mb-1">
                  <span className={`text-xs uppercase tracking-wider ${family.labelClass}`}>{family.label}</span>
                  <button
                    onClick={() => toggleFamily(family)}
                    className="text-xs text-faint hover:text-cyan-300"
                  >
                    {allSelected ? 'clear' : `select all (${eligible.length})`}
                  </button>
                </div>
                <div className="flex gap-1 flex-wrap">
                  {family.modes.map(m => {
                    const active = selectedModes.has(m);
                    const offReason = targetUnsupported.get(m);
                    return (
                      <button
                        key={m}
                        onClick={() => toggleMode(m)}
                        disabled={offReason != null}
                        title={offReason}
                        className={`px-2 py-0.5 text-xs border transition-colors rounded-sm ${
                          offReason != null
                            ? 'border-gray-800 text-gray-600 line-through cursor-not-allowed'
                            : classForMode(m, active)
                        }`}
                      >
                        {modeLabel(m)}
                      </button>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>

        {/* Step 2: TARGET */}
        <div className="mb-5">
          <div className="flex items-baseline gap-2 mb-2">
            <span className={stepMarkerClass(Boolean(selectedTargetId))}>2</span>
            <span className="text-xs text-gray-200 font-medium">Against which target?</span>
            <span className="text-xs text-gray-400 ml-auto">{deployments.length} deployed</span>
          </div>

          {selectedDeployment ? (
            <div className="flex items-center gap-2 px-3 py-2 bg-cyan-500/5 border border-cyan-500/40">
              <span className={`w-2 h-2 rounded-full ${deploymentStatusDot(selectedDeployment.status)}`} />
              <span className="text-xs text-gray-200">{selectedDeployment.name}</span>
              <span className="text-xs text-gray-400">
                {selectedDeployment.config?.endpoints?.[0]?.provider && `· ${selectedDeployment.config.endpoints[0].provider}`}
                {deploymentRegion(selectedDeployment) && ` ${deploymentRegion(selectedDeployment)}`}
              </span>
              <button
                onClick={() => { setSelectedTargetId(''); setTargetSearch(''); setTargetPopoverOpen(true); targetInputRef.current?.focus(); }}
                className="ml-auto text-xs text-gray-400 hover:text-cyan-300"
              >
                change
              </button>
            </div>
          ) : (
            <div className="relative">
              <input
                ref={targetInputRef}
                type="text"
                value={targetSearch}
                onChange={e => { setTargetSearch(e.target.value); setTargetPopoverOpen(true); }}
                onFocus={() => setTargetPopoverOpen(true)}
                placeholder={loading ? 'loading deployed targets…' : 'search by name, region, or cloud — press / to focus'}
                className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
              />
              <span className="absolute right-2 top-1/2 -translate-y-1/2 text-xs text-faint">
                <span className="px-1 border border-gray-700 rounded">/</span>
              </span>
              {targetPopoverOpen && filteredTargets.length > 0 && (
                <div className="mt-1 border border-gray-700 max-h-60 overflow-y-auto bg-[var(--bg-surface)]">
                  {filteredTargets.map(dep => (
                    <button
                      key={dep.deployment_id}
                      onClick={() => { setSelectedTargetId(dep.deployment_id); setTargetPopoverOpen(false); }}
                      className="w-full text-left grid items-center gap-2 px-3 py-2 text-xs hover:bg-cyan-500/[.06]"
                      style={{ gridTemplateColumns: '10px 1fr auto' }}
                    >
                      <span className={`w-2 h-2 rounded-full ${deploymentStatusDot(dep.status)}`} />
                      <div>
                        <div className="text-gray-200">{dep.name}</div>
                        <div className="text-xs text-gray-400">
                          {dep.config?.endpoints?.[0]?.provider ?? 'cloud unknown'}
                          {' · '}
                          {deploymentRegion(dep) ?? 'region unknown'}
                          {dep.config?.endpoints?.[0]?.http_stacks && dep.config.endpoints[0].http_stacks.length > 0 && ` · ${dep.config.endpoints[0].http_stacks.join(', ')}`}
                        </div>
                      </div>
                      <span className="text-xs text-gray-400">{dep.status}</span>
                    </button>
                  ))}
                </div>
              )}
              {targetPopoverOpen && !loading && filteredTargets.length === 0 && (
                <div className="mt-1 border border-gray-800 border-dashed px-3 py-4 text-center text-xs text-gray-400">
                  {targetSearch
                    ? `No targets match "${targetSearch}".`
                    : 'No deployed targets yet.'}
                  {' '}
                  <button onClick={() => navigate(`/projects/${projectId}/vms`)} className="text-cyan-400 hover:underline">
                    Deploy one →
                  </button>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Step 3: RUNNER */}
        <div className="mb-0">
          <div className="flex items-baseline gap-2 mb-1">
            <span className="w-5 h-5 rounded-full text-xs text-center leading-[18px] bg-gray-900 text-gray-400 border border-gray-700">3</span>
            <span className="text-xs text-gray-200 font-medium">Runner</span>
            <span className="text-xs text-gray-400 ml-auto">
              {runnerMode === 'auto' ? 'auto-pick' : selectedTesterId ? (testers.find(t => t.tester_id === selectedTesterId)?.name ?? 'none') : 'none selected'}
              {' · '}{runnerStats.idle} idle / {runnerStats.online} online
            </span>
          </div>
          {!runnerExpanded ? (
            <div className="text-xs text-gray-400 ml-7">
              First idle runner will execute this run.{' '}
              <button onClick={() => setRunnerExpanded(true)} className="text-cyan-400 hover:underline">pick specific →</button>
            </div>
          ) : (
            <div className="ml-7 space-y-1.5 mt-1">
              <div className="flex gap-1">
                <button
                  onClick={() => { setRunnerMode('auto'); setSelectedTesterId(null); }}
                  className={runnerChoiceClass(runnerMode === 'auto')}
                >
                  Auto-pick
                </button>
                <button
                  onClick={() => setRunnerMode('specific')}
                  className={runnerChoiceClass(runnerMode === 'specific')}
                >
                  Pick specific
                </button>
                <button onClick={() => setRunnerExpanded(false)} className="px-2.5 py-1 text-xs text-faint hover:text-gray-300 ml-auto">collapse</button>
              </div>
              {runnerMode === 'specific' && testers.length > 0 && (
                <div className="space-y-1">
                  {testers.map(row => {
                    // STRICT shared definition (running VM + connected agent):
                    // power_state alone let a dark-agent runner be pinned (#793 P3).
                    const isOnline = isOnlineTester(row);
                    const isIdle = row.allocation === 'idle';
                    const checked = selectedTesterId === row.tester_id;
                    return (
                      <label
                        key={row.tester_id}
                        className={`flex items-center gap-2 px-2.5 py-1.5 text-xs border cursor-pointer ${
                          !isOnline ? 'opacity-40 cursor-not-allowed' : 'hover:border-gray-600'
                        } ${checked ? 'border-cyan-500/50 bg-cyan-500/5' : 'border-gray-800'}`}
                      >
                        <input type="radio" name="runner" checked={checked} disabled={!isOnline} onChange={() => setSelectedTesterId(row.tester_id)} className="accent-cyan-400" />
                        <span className="text-gray-200">{row.name}</span>
                        <span className="text-xs text-gray-400">· {row.cloud}/{row.region}</span>
                        <span className={`ml-auto text-xs ${isOnline && isIdle ? 'text-green-400' : 'text-gray-400'}`}>
                          {isOnline
                            ? (isIdle ? 'idle' : row.allocation)
                            : row.power_state === 'running' ? 'agent offline' : row.power_state}
                        </span>
                      </label>
                    );
                  })}
                </div>
              )}
            </div>
          )}
        </div>

        {/* Launch bar inline */}
        <div className="flex items-center justify-between gap-3 mt-5 -mx-5 -mb-5 px-5 py-3 bg-cyan-500/5 border-t border-cyan-500/40">
          <div className="text-xs text-gray-400">
            <span className="text-gray-100">{selectedModes.size}</span> mode{selectedModes.size === 1 ? '' : 's'}
            {selectedDeployment && <> · <span className="text-gray-100">{selectedDeployment.name}</span></>}
            {' · '}{runnerMode === 'auto' ? 'auto-runner' : 'specific runner'}
          </div>
          <Button
            variant="primary"
            onClick={launchNew}
            disabled={!canLaunch || submitting}
            className="flex items-center gap-2"
          >
            {submitting ? (
              <>
                <span className="inline-block w-3 h-3 border-2 border-white/30 border-t-white rounded-full motion-safe:animate-spin" />
                Launching…
              </>
            ) : (
              <>
                ▶ Launch
                <span className="px-1 py-0.5 text-xs bg-black/25 border border-white/20 rounded">⏎</span>
              </>
            )}
          </Button>
        </div>
      </div>

      {/* Shortcuts hint — static footer so it can't collide with the fixed
          perf pill in the same corner (audit F11). */}
      <div className="mt-4 text-right text-xs text-faint select-none">
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">R</span> rerun last ·{' '}
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">1</span>-<span className="px-1 bg-gray-900 border border-gray-800 rounded">5</span> recent ·{' '}
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">/</span> search ·{' '}
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">⏎</span> launch
      </div>
    </div>
  );
}
