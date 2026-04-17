import { useState, useEffect, useMemo, useCallback, useRef } from 'react';
import { useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import { testersApi, type TesterRow } from '../api/testers';
import type { Deployment, TestRun, TestConfigCreate, Workload } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { familyOf } from '../components/common/mode-family';

// ── Mode families (source of truth is ModeChip.tsx) ────────────────────

interface ModeFamilyDef {
  id: 'net' | 'http' | 'thru' | 'page';
  label: string;
  modes: string[];
  activeClass: string;
  labelClass: string;
}

const MODE_FAMILIES: ModeFamilyDef[] = [
  {
    id: 'net',
    label: 'NETWORK',
    modes: ['tcp', 'dns', 'tls', 'tlsresume', 'nativetls', 'udp'],
    activeClass: 'bg-green-400/[.14] text-green-300 border-green-400/50',
    labelClass: 'text-green-300',
  },
  {
    id: 'http',
    label: 'HTTP',
    modes: ['http1', 'http2', 'http3', 'curl'],
    activeClass: 'bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50',
    labelClass: 'text-cyan-300',
  },
  {
    id: 'thru',
    label: 'THROUGHPUT',
    modes: ['download', 'upload', 'downloadh1', 'downloadh2', 'downloadh3'],
    activeClass: 'bg-violet-400/[.16] text-violet-300 border-violet-400/55',
    labelClass: 'text-violet-300',
  },
  {
    id: 'page',
    label: 'PAGE-LOAD',
    modes: ['pageload1', 'pageload2', 'pageload3'],
    activeClass: 'bg-amber-400/[.14] text-amber-300 border-amber-400/50',
    labelClass: 'text-amber-300',
  },
];

const FAMILY_BY_ID = new Map(MODE_FAMILIES.map(f => [f.id, f]));

// Modes that measure throughput — they need explicit payload sizes, otherwise
// the agent gets an empty list and the run completes with zero data moved.
const THROUGHPUT_MODES = new Set(['download', 'upload', 'downloadh1', 'downloadh2', 'downloadh3']);

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
  { id: 'thruput',  label: '★ Throughput',     modes: ['downloadh1','downloadh2','downloadh3'],     desc: 'h1/h2/h3 download' },
  { id: 'full',     label: '★ Full 18-mode',   modes: MODE_FAMILIES.flatMap(f => f.modes),          desc: 'everything' },
];

function classForMode(mode: string, active: boolean): string {
  const family = familyOf(mode);
  if (!active) return 'border-gray-700 text-gray-500 hover:text-gray-300 hover:border-gray-600';
  return FAMILY_BY_ID.get(family as ModeFamilyDef['id'])?.activeClass
    ?? 'bg-gray-700/30 text-gray-400 border-gray-700';
}

function relTime(iso: string | null): string {
  if (!iso) return '—';
  const diff = Date.now() - new Date(iso).getTime();
  if (diff < 60_000) return `${Math.floor(diff / 1000)}s ago`;
  if (diff < 3600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86400_000) return `${Math.floor(diff / 3600_000)}h ago`;
  return `${Math.floor(diff / 86400_000)}d ago`;
}

function deploymentStatusDot(status: string): string {
  switch (status) {
    case 'running': return 'bg-green-400';
    case 'stopped': case 'stopping': return 'bg-gray-500';
    case 'error': case 'failed':     return 'bg-red-400';
    case 'creating': case 'starting': return 'bg-amber-400';
    default: return 'bg-gray-600';
  }
}

// ── Component ──────────────────────────────────────────────────────────

export function NetworkTestPage() {
  const { projectId } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  usePageTitle('New Network Test');

  // Data
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [recentRuns, setRecentRuns] = useState<TestRun[]>([]);
  const [loading, setLoading] = useState(true);

  // Form state — intent-first: modes → target → runner
  const [selectedModes, setSelectedModes] = useState<Set<string>>(new Set());
  const [activePreset, setActivePreset] = useState<string | null>(null);
  const [payloadSizes, setPayloadSizes] = useState<Set<number>>(new Set(DEFAULT_PAYLOADS));
  const [selectedTargetId, setSelectedTargetId] = useState<string>('');
  const [targetSearch, setTargetSearch] = useState('');
  const [targetPopoverOpen, setTargetPopoverOpen] = useState(false);
  const [runnerMode, setRunnerMode] = useState<'auto' | 'specific'>('auto');
  const [selectedTesterId, setSelectedTesterId] = useState<string | null>(null);
  const [runnerExpanded, setRunnerExpanded] = useState(false);

  // Scope tab for recent runs
  const [scopeTab, setScopeTab] = useState<'all' | 'this' | 'mine'>('all');

  const [submitting, setSubmitting] = useState(false);
  const targetInputRef = useRef<HTMLInputElement>(null);
  const formRef = useRef<HTMLDivElement>(null);

  // ── Data loading ─────────────────────────────────────────────────────

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    Promise.all([
      api.getDeployments(projectId, { limit: 50 }).catch(() => [] as Deployment[]),
      testersApi.listTesters(projectId).catch(() => [] as TesterRow[]),
      api.listTestRuns(projectId, { endpoint_kind: 'network', limit: 5 }).catch(() => [] as TestRun[]),
    ]).then(([deps, rnrs, runs]) => {
      if (cancelled) return;
      setDeployments(deps);
      setTesters(rnrs);
      setRecentRuns(runs);
      setLoading(false);
    });
    return () => { cancelled = true; };
  }, [projectId]);

  // ── Derived ──────────────────────────────────────────────────────────

  const lastRun = recentRuns[0] ?? null;
  const selectedDeployment = useMemo(
    () => deployments.find(d => d.deployment_id === selectedTargetId) ?? null,
    [deployments, selectedTargetId],
  );
  const runnerStats = useMemo(() => {
    const online = testers.filter(t => t.power_state === 'running');
    const idle = online.filter(t => t.allocation === 'idle');
    return { online: online.length, idle: idle.length };
  }, [testers]);

  const filteredTargets = useMemo(() => {
    const q = targetSearch.trim().toLowerCase();
    if (!q) return deployments;
    return deployments.filter(d =>
      d.name.toLowerCase().includes(q)
      || d.config?.endpoints?.[0]?.provider?.toLowerCase().includes(q)
      || d.config?.endpoints?.[0]?.region?.toLowerCase().includes(q),
    );
  }, [deployments, targetSearch]);

  const filteredRecent = useMemo(() => {
    if (scopeTab === 'this' && selectedDeployment) {
      // TestRun doesn't expose target id directly; filter by config_name contains target name as a best-effort
      return recentRuns.filter(r => r.config_name?.toLowerCase().includes(selectedDeployment.name.toLowerCase()));
    }
    // 'mine' would require current-user filter; backend doesn't expose user_id on TestRun yet, so fall back to all
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
      const allSelected = family.modes.every(m => next.has(m));
      if (allSelected) family.modes.forEach(m => next.delete(m));
      else family.modes.forEach(m => next.add(m));
      return next;
    });
  }, []);

  const applyPreset = useCallback((presetId: string) => {
    const preset = MODE_PRESETS.find(p => p.id === presetId);
    if (!preset) return;
    setActivePreset(presetId);
    setSelectedModes(new Set(preset.modes));
  }, []);

  const clearModes = useCallback(() => {
    setActivePreset(null);
    setSelectedModes(new Set());
  }, []);

  // ── Rerun ────────────────────────────────────────────────────────────

  const rerunConfig = useCallback(async (configId: string) => {
    if (submitting) return;
    setSubmitting(true);
    try {
      const run = await api.launchTestConfig(configId);
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
      const cfg = await api.getTestConfig(run.test_config_id);
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
  }, [addToast]);

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
        endpoint: { kind: 'proxy', proxy_endpoint_id: selectedDeployment.deployment_id },
        workload,
      };
      const created = await api.createTestConfig(projectId, config);
      const run = await api.launchTestConfig(created.id, selectedTesterId ?? undefined);
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
        <p className="text-xs text-gray-500 mt-1">
          Run network primitives against a deployed target. Most runs repeat — use the hero below for the fastest path.
        </p>
      </div>

      {/* ─── HERO: Rerun last ──────────────────────────────────────── */}
      {lastRun && lastRun.modes && lastRun.modes.length > 0 && (
        <div
          className="grid items-center gap-3 mb-3 px-5 py-3 border border-cyan-500/40 bg-cyan-500/5"
          style={{ gridTemplateColumns: 'auto 1fr auto auto', borderLeft: '3px solid rgb(34, 211, 238)' }}
        >
          <div className="text-cyan-400 text-xl leading-none">↻</div>
          <div>
            <div className="text-[10px] uppercase tracking-wider font-mono text-gray-500">Repeat last run</div>
            <div className="text-sm font-mono text-gray-200 mt-0.5">
              <span className="text-cyan-300">{lastRun.config_name ?? lastRun.id.slice(0, 8)}</span>
            </div>
            <div className="flex items-center gap-1 flex-wrap mt-1 text-[11px] text-gray-500">
              {lastRun.modes.slice(0, 8).map(m => (
                <span
                  key={m}
                  className={`inline-flex items-center px-1.5 py-0.5 text-[10px] font-mono leading-tight border rounded-sm ${classForMode(m, true)}`}
                >
                  {m}
                </span>
              ))}
              {lastRun.modes.length > 8 && (
                <span className="text-[10px] text-gray-600">+{lastRun.modes.length - 8}</span>
              )}
              <span className="ml-2">· {relTime(lastRun.finished_at ?? lastRun.started_at ?? lastRun.created_at)}</span>
              {lastRun.status === 'completed' && (
                <span className="ml-2 text-green-400">· {lastRun.success_count}/{lastRun.success_count + lastRun.failure_count} ✓</span>
              )}
              <span className="ml-2">
                · <button onClick={() => navigate(`/projects/${projectId}/runs/${lastRun.id}`)} className="text-cyan-400 hover:underline">view last run</button>
              </span>
            </div>
          </div>
          <button
            onClick={() => tweakFromRun(lastRun)}
            className="px-2.5 py-1.5 text-[11px] border border-gray-700 text-gray-400 hover:border-cyan-500/40 hover:text-cyan-300 transition-colors"
            title="Load this config into the form below"
          >
            ✎ tweak &amp; run
          </button>
          <button
            onClick={() => rerunConfig(lastRun.test_config_id)}
            disabled={submitting}
            className="px-4 py-2 bg-cyan-500 hover:bg-cyan-400 disabled:opacity-50 text-gray-900 text-xs font-semibold transition-colors flex items-center gap-2"
          >
            ▶ Run again
            <span className="px-1.5 py-0.5 text-[9px] font-mono bg-gray-900/30 border border-gray-900/40 rounded">R</span>
          </button>
        </div>
      )}

      {/* ─── Recent runs ─────────────────────────────────────────── */}
      {recentRuns.length > 1 && (
        <div className="mb-4">
          <div className="flex items-baseline justify-between mb-1.5">
            <h3 className="text-xs font-semibold text-gray-300 tracking-wider">Recent runs</h3>
            <span className="text-[10px] font-mono text-gray-600">rerun any row or click → for detail</span>
          </div>
          <div className="flex items-center gap-1 mb-2 text-[11px]">
            {([
              { id: 'all' as const, label: 'All', count: recentRuns.length },
              { id: 'this' as const, label: 'This target only', count: selectedDeployment ? filteredRecent.length : 0 },
              { id: 'mine' as const, label: 'Mine only', count: recentRuns.length },
            ]).map(t => (
              <button
                key={t.id}
                onClick={() => setScopeTab(t.id)}
                disabled={t.id === 'this' && !selectedDeployment}
                className={`px-2.5 py-0.5 border transition-colors ${
                  scopeTab === t.id
                    ? 'border-cyan-500/40 text-cyan-300 bg-cyan-500/5'
                    : 'border-transparent text-gray-500 hover:text-gray-300'
                } disabled:opacity-40 disabled:cursor-not-allowed`}
              >
                {t.label} <span className="text-[10px] text-gray-600 font-mono">· {t.count}</span>
              </button>
            ))}
            <div className="flex-1" />
            <button onClick={() => navigate(`/projects/${projectId}/runs`)} className="text-cyan-400 hover:underline">view all →</button>
          </div>

          <div className="border-t border-gray-800/50">
            {filteredRecent.slice(0, 5).map((run, i) => {
              const modes = run.modes ?? [];
              const ok = run.status === 'completed' && run.failure_count === 0;
              return (
                <div
                  key={run.id}
                  className="grid items-center gap-3 py-2 px-3 border-b border-gray-800/50 hover:bg-cyan-500/[.03]"
                  style={{ gridTemplateColumns: '28px 1fr auto auto 100px' }}
                >
                  <span className="inline-block px-1.5 py-0.5 text-[10px] font-mono text-gray-500 bg-gray-900 border border-gray-800 rounded text-center">
                    {i + 1}
                  </span>
                  <div className="min-w-0">
                    <span className="text-xs font-mono text-gray-200">{run.config_name ?? run.id.slice(0, 8)}</span>
                    <span className="text-[10px] text-gray-500 ml-2">{relTime(run.finished_at ?? run.started_at ?? run.created_at)}</span>
                  </div>
                  <div className="flex gap-1 flex-wrap">
                    {modes.slice(0, 6).map(m => (
                      <span
                        key={m}
                        className={`inline-flex items-center px-1.5 py-0.5 text-[9px] font-mono leading-tight border rounded-sm ${classForMode(m, true)}`}
                      >
                        {m}
                      </span>
                    ))}
                    {modes.length > 6 && <span className="text-[9px] text-gray-600">+{modes.length - 6}</span>}
                  </div>
                  <span className={`text-[11px] font-mono ${ok ? 'text-green-400' : 'text-red-400'}`}>
                    {ok
                      ? `${run.success_count}/${run.success_count} ✓`
                      : `${run.success_count}/${run.success_count + run.failure_count} · ${run.failure_count} fail`}
                  </span>
                  <div className="text-right text-[11px]">
                    <button onClick={() => rerunConfig(run.test_config_id)} disabled={submitting} className="text-cyan-400 hover:underline disabled:opacity-50" title={`rerun (key ${i + 1})`}>↻ rerun</button>
                    <button onClick={() => tweakFromRun(run)} className="text-gray-500 hover:text-cyan-300 ml-2" title="load into form">✎</button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* ─── OR DIVIDER ──────────────────────────────────────────── */}
      {recentRuns.length > 0 && (
        <div className="flex items-center gap-3 my-6 text-[11px] uppercase tracking-widest text-gray-600">
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
            <span className={`w-5 h-5 rounded-full text-[10px] font-mono text-center leading-[18px] border ${selectedModes.size > 0 ? 'bg-cyan-500 text-gray-900 border-cyan-500' : 'bg-gray-900 text-gray-500 border-gray-700'}`}>1</span>
            <span className="text-xs text-gray-200 font-medium">What are you testing?</span>
            <span className="text-[11px] text-gray-500 ml-auto">
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
                  className={`px-2.5 py-1 text-[11px] font-mono border transition-colors ${
                    active
                      ? 'border-cyan-500 text-cyan-300 bg-cyan-500/10'
                      : 'border-gray-800 border-dashed text-gray-500 hover:text-cyan-300 hover:border-cyan-500/40 hover:border-solid'
                  }`}
                  title={p.desc}
                >
                  {p.label}
                </button>
              );
            })}
            {selectedModes.size > 0 && (
              <button onClick={clearModes} className="px-2.5 py-1 text-[11px] font-mono text-gray-600 hover:text-gray-300">clear all</button>
            )}
          </div>

          {/* Payload sizes — only shown when any throughput mode is active, since
              other modes ignore payload. Keeps the form compact by default. */}
          {[...selectedModes].some(m => THROUGHPUT_MODES.has(m)) && (
            <div className="mb-2 px-2 py-1.5 border border-violet-400/30 bg-violet-500/5 rounded-sm">
              <div className="flex items-center justify-between mb-1">
                <span className="text-[10px] uppercase tracking-wider font-mono text-violet-300">
                  PAYLOAD SIZES <span className="text-gray-500 normal-case tracking-normal">· download/upload run once per selected size</span>
                </span>
                <span className="text-[10px] font-mono text-gray-500">
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
                      className={`px-2 py-0.5 text-[11px] font-mono border transition-colors rounded-sm ${
                        active
                          ? 'bg-violet-400/[.16] text-violet-200 border-violet-400/55'
                          : 'border-gray-700 text-gray-500 hover:text-gray-300 hover:border-gray-600'
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
            const allSelected = family.modes.every(m => selectedModes.has(m));
            return (
              <div key={family.id} className="mb-2">
                <div className="flex items-center justify-between mb-1">
                  <span className={`text-[10px] uppercase tracking-wider font-mono ${family.labelClass}`}>{family.label}</span>
                  <button
                    onClick={() => toggleFamily(family)}
                    className="text-[10px] font-mono text-gray-600 hover:text-cyan-300"
                  >
                    {allSelected ? 'clear' : `select all (${family.modes.length})`}
                  </button>
                </div>
                <div className="flex gap-1 flex-wrap">
                  {family.modes.map(m => {
                    const active = selectedModes.has(m);
                    return (
                      <button
                        key={m}
                        onClick={() => toggleMode(m)}
                        className={`px-2 py-0.5 text-[11px] font-mono border transition-colors rounded-sm ${classForMode(m, active)}`}
                      >
                        {m}
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
            <span className={`w-5 h-5 rounded-full text-[10px] font-mono text-center leading-[18px] border ${selectedTargetId ? 'bg-cyan-500 text-gray-900 border-cyan-500' : 'bg-gray-900 text-gray-500 border-gray-700'}`}>2</span>
            <span className="text-xs text-gray-200 font-medium">Against which target?</span>
            <span className="text-[11px] text-gray-500 ml-auto">{deployments.length} deployed</span>
          </div>

          {selectedDeployment ? (
            <div className="flex items-center gap-2 px-3 py-2 bg-cyan-500/5 border border-cyan-500/40">
              <span className={`w-2 h-2 rounded-full ${deploymentStatusDot(selectedDeployment.status)}`} />
              <span className="text-xs font-mono text-gray-200">{selectedDeployment.name}</span>
              <span className="text-[11px] text-gray-500">
                {selectedDeployment.config?.endpoints?.[0]?.provider && `· ${selectedDeployment.config.endpoints[0].provider}`}
                {selectedDeployment.config?.endpoints?.[0]?.region && ` ${selectedDeployment.config.endpoints[0].region}`}
              </span>
              <button
                onClick={() => { setSelectedTargetId(''); setTargetSearch(''); setTargetPopoverOpen(true); targetInputRef.current?.focus(); }}
                className="ml-auto text-[11px] text-gray-500 hover:text-cyan-300"
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
                className="w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 font-mono focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
              />
              <span className="absolute right-2 top-1/2 -translate-y-1/2 text-[10px] font-mono text-gray-600">
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
                        <div className="font-mono text-gray-200">{dep.name}</div>
                        <div className="text-[10px] text-gray-500">
                          {dep.config?.endpoints?.[0]?.provider ?? '?'}
                          {' · '}
                          {dep.config?.endpoints?.[0]?.region ?? '?'}
                          {dep.config?.endpoints?.[0]?.http_stacks && dep.config.endpoints[0].http_stacks.length > 0 && ` · ${dep.config.endpoints[0].http_stacks.join(', ')}`}
                        </div>
                      </div>
                      <span className="text-[10px] text-gray-500 font-mono">{dep.status}</span>
                    </button>
                  ))}
                </div>
              )}
              {targetPopoverOpen && !loading && filteredTargets.length === 0 && (
                <div className="mt-1 border border-gray-800 border-dashed px-3 py-4 text-center text-xs text-gray-500">
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
            <span className="w-5 h-5 rounded-full text-[10px] font-mono text-center leading-[18px] bg-gray-900 text-gray-500 border border-gray-700">3</span>
            <span className="text-xs text-gray-200 font-medium">Runner</span>
            <span className="text-[11px] text-gray-500 ml-auto">
              {runnerMode === 'auto' ? 'auto-pick' : selectedTesterId ? (testers.find(t => t.tester_id === selectedTesterId)?.name ?? 'none') : 'none selected'}
              {' · '}{runnerStats.idle} idle / {runnerStats.online} online
            </span>
          </div>
          {!runnerExpanded ? (
            <div className="text-[11px] text-gray-500 ml-7">
              First idle runner will execute this run.{' '}
              <button onClick={() => setRunnerExpanded(true)} className="text-cyan-400 hover:underline">pick specific →</button>
            </div>
          ) : (
            <div className="ml-7 space-y-1.5 mt-1">
              <div className="flex gap-1">
                <button
                  onClick={() => { setRunnerMode('auto'); setSelectedTesterId(null); }}
                  className={`px-2.5 py-1 text-[11px] border transition-colors ${runnerMode === 'auto' ? 'border-cyan-500/40 text-cyan-300 bg-cyan-500/5' : 'border-gray-800 text-gray-500'}`}
                >
                  Auto-pick
                </button>
                <button
                  onClick={() => setRunnerMode('specific')}
                  className={`px-2.5 py-1 text-[11px] border transition-colors ${runnerMode === 'specific' ? 'border-cyan-500/40 text-cyan-300 bg-cyan-500/5' : 'border-gray-800 text-gray-500'}`}
                >
                  Pick specific
                </button>
                <button onClick={() => setRunnerExpanded(false)} className="px-2.5 py-1 text-[11px] text-gray-600 hover:text-gray-300 ml-auto">collapse</button>
              </div>
              {runnerMode === 'specific' && testers.length > 0 && (
                <div className="space-y-1">
                  {testers.map(row => {
                    const isOnline = row.power_state === 'running';
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
                        <span className="font-mono text-gray-200">{row.name}</span>
                        <span className="text-[10px] text-gray-500">· {row.cloud}/{row.region}</span>
                        <span className={`ml-auto text-[10px] font-mono ${isIdle ? 'text-green-400' : 'text-gray-500'}`}>
                          {isOnline ? (isIdle ? 'idle' : row.allocation) : row.power_state}
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
            <span className="text-gray-100 font-mono">{selectedModes.size}</span> mode{selectedModes.size === 1 ? '' : 's'}
            {selectedDeployment && <> · <span className="text-gray-100 font-mono">{selectedDeployment.name}</span></>}
            {' · '}{runnerMode === 'auto' ? 'auto-runner' : 'specific runner'}
          </div>
          <button
            onClick={launchNew}
            disabled={!canLaunch || submitting}
            className="px-5 py-2 bg-cyan-500 hover:bg-cyan-400 disabled:bg-gray-800 disabled:text-gray-600 text-gray-900 disabled:cursor-not-allowed text-xs font-semibold transition-colors flex items-center gap-2"
          >
            {submitting ? (
              <>
                <span className="inline-block w-3 h-3 border-2 border-gray-900/30 border-t-gray-900 rounded-full animate-spin" />
                Launching…
              </>
            ) : (
              <>
                ▶ Launch
                <span className="px-1 py-0.5 text-[9px] font-mono bg-gray-900/30 border border-gray-900/40 rounded">⏎</span>
              </>
            )}
          </button>
        </div>
      </div>

      {/* Shortcuts hint — fixed bottom-right */}
      <div className="fixed bottom-4 right-6 text-[10px] text-gray-600 font-mono pointer-events-none select-none">
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">R</span> rerun last ·{' '}
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">1</span>-<span className="px-1 bg-gray-900 border border-gray-800 rounded">5</span> recent ·{' '}
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">/</span> search ·{' '}
        <span className="px-1 bg-gray-900 border border-gray-800 rounded">⏎</span> launch
      </div>
    </div>
  );
}
