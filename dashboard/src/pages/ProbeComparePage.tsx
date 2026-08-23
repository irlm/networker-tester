import { lazy, Suspense, useCallback, useMemo, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api } from '../api/client';
import type {
  ProbeComparisonMode,
  ProbeComparisonReport,
  ProbeComparisonScore,
} from '../api/types';
import { Button } from '../components/common/Button';
import { EmptyState } from '../components/common/EmptyState';
import { ExportMenu } from '../components/common/ExportMenu';
import { PageHeader } from '../components/common/PageHeader';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';
import { formatMs1dp } from '../lib/format';
import {
  CROWN_HINT,
  CROWN_LABEL,
  colorFor,
  crownsFor,
  formatJitter,
  formatPercent,
  hasNoCrowns,
  headToHeadSentence,
  notRankedReason,
  shortLabel,
} from '../lib/probeComparison';

const ProbeCompareChart = lazy(() => import('./ProbeCompareChart'));

const WINDOWS = ['24h', '7d', '30d'] as const;
const BUCKETS = ['15m', '1h', '6h', '1d'] as const;

/** Cap mirrors ProbeComparisonEndpoints.MaxUrls — the server rejects more. */
const MAX_URLS = 8;

/**
 * The URL comparison report (#782 P3).
 *
 * The question this page exists to answer is "of the URLs I watch, which is
 * fastest, which flakes, and which is steadiest" — and, just as importantly,
 * "may I believe that answer". Every number comes from the server computed over
 * SHARED time buckets (hours in which every compared URL was actually
 * measured); this page never re-aggregates them. When the overlap is too thin
 * the scoreboard is shown greyed with the reason, instead of a ranking that
 * would really be a report on who got probed at quiet hours.
 *
 * Modes are never mixed: an http1 probe and an http3 probe of the same URL are
 * not the same race, so the server returns one scoreboard per mode and the page
 * shows a tab per mode.
 *
 * Selection lives in the query string, so a comparison is a shareable link.
 */
export function ProbeComparePage() {
  const { projectId } = useProject();
  const [params, setParams] = useSearchParams();
  const [report, setReport] = useState<ProbeComparisonReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeMode, setActiveMode] = useState<string | null>(null);

  usePageTitle('Compare URLs');

  const selected = useMemo(
    () => (params.get('urls') ?? '').split(',').filter(Boolean),
    [params],
  );
  const window_ = params.get('window') ?? '7d';
  const bucket = params.get('bucket') ?? '1h';

  const refresh = useCallback(() => {
    if (!projectId) return;
    setLoading(true);
    api
      .getProbeComparison(projectId, { urls: selected, window: window_, bucket })
      .then((r) => {
        setReport(r);
        setError(null);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, [projectId, selected, window_, bucket]);

  // Refetch on every control change (resetKey) and keep the comparison live
  // while it is open — a probe schedule ticks under it. usePolling fires an
  // immediate tick, so this is also the initial load.
  usePolling(refresh, 60_000, true, `${selected.join(',')}|${window_}|${bucket}`);

  // Follow the data: when the selection changes, the mode that was open may no
  // longer exist. Fall back to the mode with the most shared coverage — the one
  // whose ranking is worth the most.
  const modes = report?.modes ?? [];
  const currentMode: ProbeComparisonMode | null =
    modes.find((m) => m.mode === activeMode) ??
    [...modes].sort((a, b) => b.shared_buckets - a.shared_buckets)[0] ??
    null;

  const setSelection = (urls: string[]) => {
    const next = new URLSearchParams(params);
    if (urls.length) next.set('urls', urls.join(','));
    else next.delete('urls');
    setParams(next, { replace: true });
  };

  const setParam = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    next.set(key, value);
    setParams(next, { replace: true });
  };

  const toggleUrl = (url: string) => {
    setSelection(
      selected.includes(url)
        ? selected.filter((u) => u !== url)
        : selected.length >= MAX_URLS
          ? selected
          : [...selected, url],
    );
  };

  const available = report?.available ?? [];
  const exportQuery = new URLSearchParams({
    urls: selected.join(','),
    window: window_,
    bucket,
  }).toString();

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Compare URLs"
        subtitle="Which URL is fastest, most reliable and most consistent — over the buckets they were all measured in"
        action={
          projectId && currentMode && selected.length >= 2 ? (
            <ExportMenu
              path={`/projects/${projectId}/reports/probe-comparison?${exportQuery}`}
              fileBase="url-comparison"
            />
          ) : undefined
        }
      />

      {error && (
        <div className="alert alert-error mb-4">
          <p className="text-red-300 text-sm">{error}</p>
          <Button className="mt-3" onClick={refresh}>Retry</Button>
        </div>
      )}

      {/* ── Controls: what to compare, over what, at what resolution ───────── */}
      <section aria-labelledby="compare-controls" className="mb-6 rounded-lg border border-gray-800">
        <div className="border-b border-gray-800 bg-[var(--bg-surface)] p-4">
          <h2 id="compare-controls" className="text-sm font-bold text-gray-100">
            URLs to compare
          </h2>
          <p className="mt-1 text-xs text-gray-400">
            Pick at least two of the URLs this project has probed. Up to {MAX_URLS}.
          </p>
        </div>

        <div className="p-4">
          {available.length === 0 && !loading ? (
            <EmptyState
              message="No probe data in this window"
              detail="Comparison reads the URL Probe's own history. Probe a few URLs — ideally as a set, so they share a tick — and they will appear here."
              action={
                <Link
                  to={`/projects/${projectId}/probe`}
                  className="inline-block bg-cyan-600 hover:bg-cyan-500 text-[var(--bg-base)] px-4 py-1.5 rounded text-sm transition-colors"
                >
                  Go to URL Probe
                </Link>
              }
            />
          ) : (
            <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
              {available.map((a) => {
                const checked = selected.includes(a.url);
                const atCap = !checked && selected.length >= MAX_URLS;
                return (
                  <li key={a.url}>
                    {/* The checkbox itself carries the 24px target (WCAG AA):
                        axe measures the INPUT's box, and padding on a wrapping
                        label does not count toward it. */}
                    <label
                      className={`flex items-start gap-2 rounded border p-2 text-xs ${
                        checked ? 'border-cyan-500/40 bg-cyan-500/5' : 'border-gray-800'
                      } ${atCap ? 'opacity-50' : 'cursor-pointer hover:border-gray-700'}`}
                    >
                      <input
                        type="checkbox"
                        className="mt-0.5 h-6 w-6 shrink-0 accent-cyan-500"
                        checked={checked}
                        disabled={atCap}
                        onChange={() => toggleUrl(a.url)}
                      />
                      <span className="min-w-0">
                        <span className="block truncate text-gray-200" title={a.url}>
                          {shortLabel(a.url)}
                        </span>
                        <span className="block text-faint">
                          {a.sample_count} sample{a.sample_count === 1 ? '' : 's'} ·{' '}
                          {a.mode_count} mode{a.mode_count === 1 ? '' : 's'}
                        </span>
                      </span>
                    </label>
                  </li>
                );
              })}
            </ul>
          )}

          <div className="mt-4 flex flex-wrap items-center gap-4">
            <label className="flex items-center gap-2 text-xs text-gray-400">
              Window
              <select
                className="select-field"
                value={window_}
                onChange={(e) => setParam('window', e.target.value)}
              >
                {WINDOWS.map((w) => <option key={w} value={w}>{w}</option>)}
              </select>
            </label>
            <label className="flex items-center gap-2 text-xs text-gray-400">
              Bucket
              <select
                className="select-field"
                value={bucket}
                onChange={(e) => setParam('bucket', e.target.value)}
              >
                {BUCKETS.map((b) => <option key={b} value={b}>{b}</option>)}
              </select>
            </label>
            {selected.length > 0 && (
              <Button size="sm" onClick={() => setSelection([])}>Clear selection</Button>
            )}
            {selected.length === 1 && (
              <span className="text-xs text-yellow-400">Pick one more URL to compare.</span>
            )}
          </div>
        </div>
      </section>

      {loading && !report && (
        <div className="text-sm text-gray-400 motion-safe:animate-pulse">Loading comparison…</div>
      )}

      {selected.length >= 2 && report && modes.length === 0 && !loading && (
        <EmptyState
          message="Nothing to compare yet"
          detail="These URLs have no probe attempts in this window. Try a longer window, or probe them together as a set."
        />
      )}

      {currentMode && report && (
        <>
          {/* ── Mode tabs: never one pooled scoreboard ──────────────────────── */}
          {modes.length > 1 && (
            <div className="mb-4 flex flex-wrap gap-2" role="tablist" aria-label="Protocol mode">
              {modes.map((m) => (
                <button
                  key={m.mode}
                  role="tab"
                  aria-selected={m.mode === currentMode.mode}
                  onClick={() => setActiveMode(m.mode)}
                  className={`rounded border px-3 py-1.5 text-xs transition-colors ${
                    m.mode === currentMode.mode
                      ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-300'
                      : 'border-gray-800 text-gray-400 hover:border-gray-700'
                  }`}
                >
                  {m.mode}
                  <span className="ml-1.5 text-faint">{m.shared_buckets} shared</span>
                </button>
              ))}
            </div>
          )}

          <ModeReport mode={currentMode} report={report} urls={selected} />
        </>
      )}
    </div>
  );
}

/** One mode's coverage, crowns, scoreboard, chart and head-to-head record. */
function ModeReport({
  mode,
  report,
  urls,
}: {
  mode: ProbeComparisonMode;
  report: ProbeComparisonReport;
  urls: string[];
}) {
  const notRanked = notRankedReason(mode, report.window_buckets);
  const excluded = mode.coverage.filter((c) => !c.eligible);

  return (
    <>
      {/* ── Coverage first: the licence to read everything below ──────────── */}
      <div
        className={`mb-6 rounded-lg border p-4 md:p-5 ${
          mode.ranked ? 'border-gray-800 bg-[var(--bg-surface)]' : 'border-yellow-500/40 bg-yellow-500/5'
        }`}
      >
        <div className="flex flex-wrap items-baseline gap-x-6 gap-y-1">
          <span className="text-xs uppercase tracking-wider text-gray-400">Shared coverage</span>
          <span className={`text-lg font-bold ${mode.ranked ? 'text-gray-100' : 'text-yellow-300'}`}>
            {mode.shared_buckets} / {report.window_buckets} buckets
          </span>
          <span className="text-sm text-gray-400">{formatPercent(mode.coverage_ratio)}</span>
        </div>
        {notRanked ? (
          <p className="mt-2 text-sm text-yellow-300">{notRanked}</p>
        ) : (
          <p className="mt-2 text-xs text-gray-400">
            Every figure below is computed over those {mode.shared_buckets} buckets only — the ones
            in which all {mode.scores.length} URLs were measured.
          </p>
        )}
        {excluded.length > 0 && (
          <p className="mt-2 text-xs text-faint">
            Not compared — too little data in this window:{' '}
            {excluded.map((c) => `${shortLabel(c.url)} (${c.qualifying_buckets}/${report.window_buckets})`).join(', ')}
          </p>
        )}
      </div>

      {/* ── Crowns ────────────────────────────────────────────────────────── */}
      {mode.ranked && (
        <div className="mb-6">
          {hasNoCrowns(mode.crowns) ? (
            <p className="text-sm text-gray-400">
              No category had a single winner — every measured metric was a tie.
            </p>
          ) : (
            <ul className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4" aria-label="Category winners">
              {(['fastest', 'most_reliable', 'most_consistent', 'best_ttfb'] as const)
                .filter((key) => mode.crowns[key] !== null)
                .map((key) => (
                  <li
                    key={key}
                    className="rounded border border-gray-800 bg-[var(--bg-surface)] p-3"
                    title={CROWN_HINT[key]}
                  >
                    <div className="text-xs uppercase tracking-wider text-faint">
                      {CROWN_LABEL[key]}
                    </div>
                    <div
                      className="mt-1 truncate text-sm font-semibold"
                      style={{ color: colorFor(urls, mode.crowns[key]!) }}
                      title={mode.crowns[key]!}
                    >
                      {shortLabel(mode.crowns[key]!)}
                    </div>
                  </li>
                ))}
            </ul>
          )}
        </div>
      )}

      {/* ── Overlaid time series ──────────────────────────────────────────── */}
      {mode.series.length > 0 && (
        <Suspense
          fallback={
            <div
              className="mb-6 h-72 rounded border border-gray-800 motion-safe:animate-pulse"
              aria-label="Loading comparison chart"
            />
          }
        >
          <ProbeCompareChart
            points={mode.series}
            urls={urls}
            bucketSeconds={report.bucket_seconds}
          />
        </Suspense>
      )}

      {/* ── Scoreboard ────────────────────────────────────────────────────── */}
      <div className={`table-container mb-6 ${mode.ranked ? '' : 'opacity-60'}`}>
        <table className="w-full text-sm">
          <caption className="sr-only">
            {mode.ranked
              ? `Scoreboard for ${mode.mode}, over ${mode.shared_buckets} shared buckets`
              : `Unranked figures for ${mode.mode} — insufficient shared coverage`}
          </caption>
          <thead>
            <tr className="border-b border-gray-800/50 bg-[var(--bg-surface)] text-xs text-gray-400">
              <th className="px-4 py-2.5 text-left font-medium">URL</th>
              <th className="px-4 py-2.5 text-right font-medium">p50</th>
              <th className="px-4 py-2.5 text-right font-medium">p95</th>
              <th className="px-4 py-2.5 text-right font-medium">Success</th>
              <th className="px-4 py-2.5 text-right font-medium" title="p95 / p50 — lower is steadier">
                Jitter
              </th>
              <th className="px-4 py-2.5 text-right font-medium">DNS</th>
              <th className="px-4 py-2.5 text-right font-medium">TCP</th>
              <th className="px-4 py-2.5 text-right font-medium">TLS</th>
              <th className="px-4 py-2.5 text-right font-medium">TTFB</th>
              <th className="px-4 py-2.5 text-right font-medium">Samples</th>
            </tr>
          </thead>
          <tbody>
            {mode.scores.map((s: ProbeComparisonScore) => (
              <tr key={s.url} className="border-b border-gray-800/50 align-top hover:bg-gray-800/20">
                <td className="px-4 py-3">
                  <div className="flex items-center gap-2">
                    <span
                      className="inline-block h-3 w-3 shrink-0 rounded-sm"
                      style={{ backgroundColor: colorFor(urls, s.url) }}
                      aria-hidden="true"
                    />
                    <span className="truncate text-gray-200" title={s.url}>{shortLabel(s.url)}</span>
                  </div>
                  {mode.ranked && (
                    <div className="mt-1 flex flex-wrap gap-1">
                      {crownsFor(mode.crowns, s.url).map((key) => (
                        <span
                          key={key}
                          className="rounded border border-cyan-500/40 bg-cyan-500/10 px-1.5 py-0.5 text-xs text-cyan-300"
                          title={CROWN_HINT[key]}
                        >
                          {CROWN_LABEL[key]}
                        </span>
                      ))}
                    </div>
                  )}
                  {s.dominant_error_category && (
                    <div className="mt-1 text-xs text-yellow-400" title="Most common failure category over the shared buckets">
                      mostly {s.dominant_error_category}
                    </div>
                  )}
                </td>
                <td className="px-4 py-3 text-right text-xs text-gray-200">{formatMs1dp(s.median_p50_ms)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-400">{formatMs1dp(s.median_p95_ms)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-300">{formatPercent(s.success_rate)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-300">{formatJitter(s.jitter_ratio)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-400">{formatMs1dp(s.median_dns_ms)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-400">{formatMs1dp(s.median_tcp_ms)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-400">{formatMs1dp(s.median_tls_ms)}</td>
                <td className="px-4 py-3 text-right text-xs text-gray-400">{formatMs1dp(s.median_ttfb_ms)}</td>
                <td className="px-4 py-3 text-right text-xs text-faint">
                  {s.samples} / {s.shared_buckets}b
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {/* ── Head-to-head: the claim the report actually makes ─────────────── */}
      {mode.head_to_head.length > 0 && (
        <section aria-labelledby="h2h-title" className="mb-6">
          <h2 id="h2h-title" className="mb-2 text-sm font-bold text-gray-100">Head to head</h2>
          <ul className="space-y-1 text-xs text-gray-300">
            {mode.head_to_head.map((h) => (
              <li key={`${h.a}|${h.b}`} className="rounded border border-gray-800 px-3 py-2">
                <span className="text-gray-400">
                  {shortLabel(h.a)} vs {shortLabel(h.b)}:
                </span>{' '}
                {headToHeadSentence(h)}
              </li>
            ))}
          </ul>
        </section>
      )}

      {/* ── Methodology, verbatim from the response ──────────────────────── */}
      <div className="space-y-1 text-xs text-faint">
        <p>{report.methodology.shared}</p>
        <p>{report.methodology.eligibility}</p>
        <p>{report.methodology.ranking}</p>
        <p>{report.methodology.modes}</p>
        <p className="pt-1">
          Generated {new Date(report.generated_at).toLocaleString()} · window {report.window} ·
          bucket {report.bucket} · at least {report.min_samples} samples per bucket
        </p>
      </div>
    </>
  );
}
