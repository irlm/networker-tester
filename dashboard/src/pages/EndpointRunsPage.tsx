// ── EndpointRunsPage ─────────────────────────────────────────────────────
//
// Per-endpoint runs-list (v6+color mockup). Lands when the user clicks a
// deployed target from /vms. Shows endpoint metadata in the hero, preset
// cards on top to launch a new run quickly, and a history table of all
// past Network benchmark runs against this endpoint with color-coded
// mode chips.
//
// Endpoint-scoped filter for test_runs is a follow-up — for now we list
// recent network-kind runs in the project and rely on the user to scope
// via the search box. The visual structure ships now so every other piece
// has a place to land later.

import { useMemo, useEffect, useState, useCallback } from 'react';
import { useParams, Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { Deployment, TestRun } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ModeChipList } from '../components/common/ModeChip';
import { timeAgo } from '../lib/format';

// ── Saved presets ──────────────────────────────────────────────────────
//
// Stored in-memory for the MVP. Per-user preset storage is a follow-up;
// the layout is preset-aware today so presets becoming first-class is
// just wiring on top.

interface Preset {
  id: string;
  star?: boolean;
  name: string;
  desc: string;
  modes: string[];
}

const DEFAULT_PRESETS: Preset[] = [
  { id: 'quick',  star: true, name: 'Quick check',     desc: '10 runs · ~30s',                modes: ['tcp', 'dns', 'tls', 'http2'] },
  { id: 'h3',     star: true, name: 'HTTP/3 only',     desc: '10 runs · ~15s',                modes: ['http3', 'downloadh3'] },
  { id: 'thru',   star: true, name: 'Throughput sweep', desc: 'payloads 64K · 1M · 16M · ~3min', modes: ['downloadh1', 'downloadh2', 'downloadh3', 'uploadh1', 'uploadh2', 'uploadh3'] },
  { id: 'full',               name: 'Full 18-mode',     desc: '50 runs · ~8min',                modes: ['tcp','dns','tls','tlsresume','nativetls','udp','http1','http2','http3','curl','download','upload','downloadh1','downloadh2','downloadh3','pageload1','pageload2','pageload3'] },
];

/** Compare two mode arrays without mutating; ordering-insensitive. */
function sameModes(a: string[] = [], b: string[] = []): boolean {
  if (a.length !== b.length) return false;
  const sa = [...a].sort().join(',');
  const sb = [...b].sort().join(',');
  return sa === sb;
}

function matchPreset(modes: string[] | undefined): Preset | null {
  if (!modes || modes.length === 0) return null;
  return DEFAULT_PRESETS.find((p) => sameModes(p.modes, modes)) ?? null;
}

// ── Component ──────────────────────────────────────────────────────────

export function EndpointRunsPage() {
  const { projectId } = useProject();
  const { endpointId } = useParams<{ endpointId: string }>();
  const navigate = useNavigate();
  usePageTitle('Endpoint runs');

  const [deployment, setDeployment] = useState<Deployment | null>(null);
  const [runs, setRuns] = useState<TestRun[]>([]);
  const [search, setSearch] = useState('');
  const [presetFilter, setPresetFilter] = useState<string>('all');

  // Load the deployment metadata for the hero.
  useEffect(() => {
    if (!endpointId || !projectId) return;
    api.getDeployment(projectId, endpointId).then(setDeployment).catch(() => setDeployment(null));
  }, [projectId, endpointId]);

  // Load network-kind runs and refresh on poll. Endpoint-scoped filter is
  // a backend follow-up; we currently ask for all and trust the search UI.
  const loadRuns = useCallback(() => {
    if (!projectId) return;
    api
      .listTestRuns(projectId, { endpoint_kind: 'network', limit: 50 })
      .then((rows) => setRuns(rows ?? []))
      .catch(() => {});
  }, [projectId]);
  useEffect(loadRuns, [loadRuns]);
  usePolling(loadRuns, 15000);

  // ── Derived filters ──────────────────────────────────────────────────
  const filteredRuns = useMemo(() => {
    let rows = runs;
    if (presetFilter !== 'all') {
      if (presetFilter === 'custom') {
        rows = rows.filter((r) => !matchPreset(r.modes));
      } else {
        const p = DEFAULT_PRESETS.find((x) => x.id === presetFilter);
        rows = p ? rows.filter((r) => sameModes(r.modes, p.modes)) : rows;
      }
    }
    if (search.trim().length > 0) {
      const q = search.trim().toLowerCase();
      rows = rows.filter(
        (r) =>
          r.id.toLowerCase().includes(q) ||
          (r.config_name ?? '').toLowerCase().includes(q) ||
          (r.modes ?? []).some((m) => m.toLowerCase().includes(q)),
      );
    }
    return rows;
  }, [runs, presetFilter, search]);

  // Last run = most recent across all runs (not the filtered set, since
  // the filter is for display only — "rerun last" should always rerun the
  // single most recent benchmark).
  const lastRun = runs[0] ?? null;

  // ── Run-launch helpers ───────────────────────────────────────────────
  // For now both "Run preset" and "Rerun this exact config" navigate to
  // the Network test wizard with modes carried in the URL. Wizard prefill
  // is wired separately; today this is a sensible drop-off point.
  const launchModes = useCallback(
    (modes: string[]) => {
      const qs = new URLSearchParams({ modes: modes.join(',') }).toString();
      navigate(`/projects/${projectId}/tests/new?${qs}`);
    },
    [navigate, projectId],
  );

  // ── Render ───────────────────────────────────────────────────────────

  const ip = deployment?.endpoint_ips?.[0] ?? '—';
  const provider = deployment?.config?.endpoints?.[0]?.provider ?? '';
  const region =
    deployment?.config?.endpoints?.[0]?.region ?? deployment?.provider_summary ?? '';
  const stacks = deployment?.config?.endpoints?.[0]?.http_stacks ?? [];
  const vmSize =
    deployment?.config?.endpoints?.[0]?.vm_size ??
    deployment?.config?.endpoints?.[0]?.instance_type ??
    deployment?.config?.endpoints?.[0]?.machine_type ??
    '';

  return (
    <div className="p-4 md:p-6 max-w-6xl mx-auto">
      <Breadcrumb
        items={[
          { label: 'Network', to: `/projects/${projectId}/tests/new` },
          { label: deployment?.name ?? endpointId ?? 'endpoint' },
        ]}
      />

      {/* ── Hero: endpoint metadata + page-level CTAs ─────────────── */}
      <div className="grid grid-cols-1 md:grid-cols-[1fr_auto] gap-4 items-start mt-2 mb-4">
        <div>
          <span className="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs border border-green-500/40 bg-green-500/10 text-green-300 rounded">
            <span className="w-1.5 h-1.5 rounded-full bg-green-400" />
            online
          </span>
          <h2 className="text-xl font-bold text-gray-100 font-mono mt-2">
            {deployment?.name ?? 'endpoint'}
            {ip !== '—' && <span className="text-cyan-400 text-sm ml-2">· {ip}</span>}
          </h2>
          <div className="flex flex-wrap gap-3 mt-1 text-[11px] font-mono text-gray-500">
            {provider && <span>{provider} · {region}</span>}
            {vmSize && <span>· {vmSize}</span>}
            {stacks.length > 0 && <span className="text-cyan-400">· {stacks.join(', ')}</span>}
            <span>· {runs.length} runs</span>
          </div>
        </div>
      </div>

      {/* ── Run a test ─────────────────────────────────────────────── */}
      <div className="flex items-baseline justify-between mb-2">
        <h3 className="text-sm font-semibold text-gray-200">Run a test</h3>
        <span className="text-[11px] text-gray-500">
          pick a saved config or build a custom one · color:
          <span className="ml-2 px-1.5 py-0.5 border rounded-sm bg-green-400/[.14] text-green-300 border-green-400/50 font-mono text-[9px]">net</span>
          <span className="ml-1 px-1.5 py-0.5 border rounded-sm bg-cyan-400/[.14] text-cyan-300 border-cyan-400/50 font-mono text-[9px]">http</span>
          <span className="ml-1 px-1.5 py-0.5 border rounded-sm bg-violet-400/[.16] text-violet-300 border-violet-400/55 font-mono text-[9px]">thru</span>
          <span className="ml-1 px-1.5 py-0.5 border rounded-sm bg-amber-400/[.14] text-amber-300 border-amber-400/50 font-mono text-[9px]">page</span>
        </span>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-2 mb-4">
        {DEFAULT_PRESETS.map((p) => (
          <div key={p.id} className="p-3 border border-gray-800 bg-[var(--bg-surface)]">
            <div className="text-sm text-gray-100 font-medium">
              {p.star && <span className="text-amber-300 mr-1">★</span>}
              {p.name}
            </div>
            <div className="text-[10px] text-gray-500 font-mono mt-1">{p.desc}</div>
            <div className="mt-2"><ModeChipList modes={p.modes} max={6} /></div>
            <button
              type="button"
              onClick={() => launchModes(p.modes)}
              className="mt-3 px-3 py-1 bg-cyan-600 hover:bg-cyan-500 text-white text-xs font-medium"
            >
              ▶ Run
            </button>
          </div>
        ))}
      </div>

      <div className="flex justify-between items-center mb-4 text-xs text-gray-500">
        <button
          type="button"
          onClick={() => navigate(`/projects/${projectId}/tests/new`)}
          className="px-3 py-1 border border-cyan-500/40 text-cyan-300 hover:bg-cyan-500/10"
        >
          + Custom test (pick modes manually)
        </button>
        {lastRun && (
          <span>
            last run:{' '}
            <Link to={`/projects/${projectId}/runs/${lastRun.id}`} className="text-cyan-400 hover:underline font-mono">
              {lastRun.id.slice(0, 8)}
            </Link>{' '}
            · {timeAgo(lastRun.created_at)} ·{' '}
            <button
              type="button"
              onClick={() => launchModes(lastRun.modes ?? [])}
              className="text-cyan-400 hover:underline"
            >
              ↻ rerun
            </button>
          </span>
        )}
      </div>

      {/* ── History ───────────────────────────────────────────────── */}
      <div className="flex items-baseline justify-between mt-6 mb-2">
        <h3 className="text-sm font-semibold text-gray-200">History</h3>
        <span className="text-[11px] text-gray-500">
          {filteredRuns.length} of {runs.length} runs ·{' '}
          <Link to={`/projects/${projectId}/runs`} className="text-cyan-400 hover:underline">
            view all →
          </Link>
        </span>
      </div>

      <div className="flex items-center gap-2 mb-3">
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="search runs…"
          className="bg-[var(--bg-base)] border border-gray-700 px-3 py-1 text-xs font-mono text-gray-200 w-60 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
        />
        <select
          value={presetFilter}
          onChange={(e) => setPresetFilter(e.target.value)}
          className="bg-[var(--bg-base)] border border-gray-700 px-2 py-1 text-xs font-mono text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          <option value="all">all configs</option>
          {DEFAULT_PRESETS.map((p) => (
            <option key={p.id} value={p.id}>
              {p.star ? '★ ' : ''}
              {p.name}
            </option>
          ))}
          <option value="custom">custom</option>
        </select>
      </div>

      {filteredRuns.length === 0 ? (
        <div className="border border-dashed border-gray-800 p-8 text-center text-xs text-gray-500">
          {runs.length === 0 ? 'No network runs yet — pick a preset above to start.' : 'No runs match the current filter.'}
        </div>
      ) : (
        <div className="border border-gray-800">
          <table className="w-full text-xs font-mono">
            <thead>
              <tr className="text-gray-500 text-[10px] uppercase tracking-wider bg-[var(--bg-raised)]">
                <th className="text-left px-3 py-2 font-medium">Run</th>
                <th className="text-left px-3 py-2 font-medium">When</th>
                <th className="text-left px-3 py-2 font-medium">Config · modes</th>
                <th className="text-right px-3 py-2 font-medium">Result</th>
                <th className="text-right px-3 py-2 font-medium" />
              </tr>
            </thead>
            <tbody>
              {filteredRuns.map((run) => {
                const preset = matchPreset(run.modes);
                const total = run.success_count + run.failure_count;
                return (
                  <tr key={run.id} className="border-t border-gray-800/60 hover:bg-cyan-500/[.04]">
                    <td className="px-3 py-2">
                      <Link
                        to={`/projects/${projectId}/runs/${run.id}`}
                        className="text-cyan-400 hover:underline"
                      >
                        {run.id.slice(0, 8)}
                      </Link>
                    </td>
                    <td className="px-3 py-2 text-gray-500">{timeAgo(run.created_at)}</td>
                    <td className="px-3 py-2">
                      <div className="flex items-center gap-2 mb-1">
                        {preset ? (
                          <span className="text-[10px] px-1.5 py-0.5 border border-cyan-500/40 text-cyan-300 rounded">
                            {preset.star && <span className="mr-0.5">★</span>}
                            {preset.name}
                          </span>
                        ) : (
                          <span className="text-[10px] px-1.5 py-0.5 border border-gray-700 text-gray-500 rounded">
                            custom
                          </span>
                        )}
                      </div>
                      <ModeChipList modes={run.modes ?? []} />
                    </td>
                    <td className="px-3 py-2 text-right">
                      {run.failure_count > 0 ? (
                        <span className="text-amber-300">
                          {run.success_count}/{total}
                          <span className="text-red-400 ml-1">· {run.failure_count} fail</span>
                        </span>
                      ) : (
                        <span className="text-green-400">{run.success_count}/{total} ✓</span>
                      )}
                    </td>
                    <td className="px-3 py-2 text-right">
                      <button
                        type="button"
                        onClick={() => launchModes(run.modes ?? [])}
                        className="text-gray-500 hover:text-cyan-400 mr-3"
                        title="Rerun this exact config"
                      >
                        ↻ rerun
                      </button>
                      <Link
                        to={`/projects/${projectId}/runs/${run.id}`}
                        className="text-gray-500 hover:text-cyan-400"
                        title="Open run details"
                      >
                        →
                      </Link>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

export default EndpointRunsPage;
