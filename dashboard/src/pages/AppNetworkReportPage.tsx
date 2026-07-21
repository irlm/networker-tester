import { useCallback, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type AppNetworkReport, type AppNetworkGroup, type AppNetworkVerdict } from '../api/client';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

// ── Brand-token colors (not raw hexes for the split legs — those come from the
// theme's cyan accent / purple brand). The split bar uses inline background
// with CSS var references so it tracks the theme.
const NETWORK_COLOR = 'var(--accent-cyan, #47bfff)';
const SERVER_COLOR = 'var(--brand-purple, #863bff)';

interface VerdictStyle {
  /** Text + accent color class for the headline. */
  text: string;
  border: string;
  bg: string;
  label: string;
}

/**
 * Verdict color coding (spec): server_bound (amber/purple — the app is the
 * problem), network_bound (cyan — the network is the problem), balanced
 * (neutral). no_data reads as muted.
 */
function verdictStyle(verdict: AppNetworkVerdict): VerdictStyle {
  switch (verdict) {
    case 'server_bound':
      return { text: 'text-purple-300', border: 'border-purple-500/40', bg: 'bg-purple-500/10', label: 'Server-bound' };
    case 'network_bound':
      return { text: 'text-cyan-300', border: 'border-cyan-500/40', bg: 'bg-cyan-500/10', label: 'Network-bound' };
    case 'balanced':
      return { text: 'text-gray-300', border: 'border-gray-700', bg: 'bg-gray-800/30', label: 'Balanced' };
    default:
      return { text: 'text-gray-500', border: 'border-gray-800', bg: 'bg-gray-900/40', label: 'No data' };
  }
}

/** Small verdict pill for the per-endpoint table. */
function VerdictBadge({ verdict }: { verdict: AppNetworkVerdict }) {
  const s = verdictStyle(verdict);
  return (
    <span className={`inline-block px-2 py-0.5 rounded text-[11px] font-medium border ${s.border} ${s.bg} ${s.text}`}>
      {s.label}
    </span>
  );
}

function fmtMs(v: number | null): string {
  return v === null ? '—' : `${v.toFixed(1)}ms`;
}

function fmtRatio(v: number | null): string {
  return v === null ? '—' : `${Math.round(v * 100)}%`;
}

/**
 * Horizontal stacked split bar: network (cyan) | server (purple), sized by the
 * two medians. Shows median values, and (when given) the p95s below.
 */
function SplitBar({
  networkMs,
  serverMs,
  p95NetworkMs,
  p95ServerMs,
}: {
  networkMs: number | null;
  serverMs: number | null;
  p95NetworkMs?: number | null;
  p95ServerMs?: number | null;
}) {
  const net = networkMs ?? 0;
  const srv = serverMs ?? 0;
  const total = net + srv;
  const hasData = total > 0;
  const netPct = hasData ? (net / total) * 100 : 0;
  const srvPct = hasData ? (srv / total) * 100 : 0;

  return (
    <div>
      <div
        className="flex h-5 w-full rounded overflow-hidden border border-gray-800 bg-gray-900"
        role="img"
        aria-label={`Latency split: network ${fmtMs(networkMs)}, server ${fmtMs(serverMs)}`}
      >
        {hasData ? (
          <>
            <div style={{ width: `${netPct}%`, backgroundColor: NETWORK_COLOR }} title={`Network ${fmtMs(networkMs)}`} />
            <div style={{ width: `${srvPct}%`, backgroundColor: SERVER_COLOR }} title={`Server ${fmtMs(serverMs)}`} />
          </>
        ) : (
          <div className="w-full bg-gray-800/40" />
        )}
      </div>
      <div className="flex justify-between mt-1 text-[11px] font-mono">
        <span className="text-cyan-400">
          net {fmtMs(networkMs)}
          {p95NetworkMs != null && <span className="text-gray-600"> · p95 {fmtMs(p95NetworkMs)}</span>}
        </span>
        <span className="text-purple-400">
          srv {fmtMs(serverMs)}
          {p95ServerMs != null && <span className="text-gray-600"> · p95 {fmtMs(p95ServerMs)}</span>}
        </span>
      </div>
    </div>
  );
}

/**
 * Application Network Performance report — THE sellable screen. Answers "is my
 * slowness the app or the network?" for LagHound SDK endpoints (sdkprobe runs).
 * member-read.
 */
export function AppNetworkReportPage() {
  const { projectId } = useProject();
  const [report, setReport] = useState<AppNetworkReport | null>(null);
  const [loading, setLoading] = useState(true);

  usePageTitle('App Network');

  const refresh = useCallback(() => {
    if (!projectId) return;
    api
      .getAppNetworkReport(projectId)
      .then((r) => {
        setReport(r);
        setLoading(false);
      })
      .catch(() => setLoading(false));
  }, [projectId]);

  usePolling(refresh, 30000);

  if (loading && !report) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Application Network Performance</h2>
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <div key={i} className="border border-gray-800 rounded p-4">
              <div className="h-4 w-48 rounded bg-gray-800/60 motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  const noData =
    !report || report.overall_verdict === 'no_data' || report.groups.length === 0 || report.attempt_count === 0;

  const overallStyle = report ? verdictStyle(report.overall_verdict) : verdictStyle('no_data');

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Application Network Performance"
        subtitle={
          report && !noData
            ? `${report.attempt_count} sdkprobe attempt${report.attempt_count === 1 ? '' : 's'} across ${report.groups.length} endpoint${report.groups.length === 1 ? '' : 's'}`
            : 'Is the slowness your application or the network?'
        }
      />

      {noData ? (
        <EmptyState
          message="No sdkprobe data yet"
          detail="This report splits request latency into network vs server for LagHound SDK endpoints. Register an SDK endpoint and run it to see whether your application or the network is the bottleneck."
          action={
            <Link
              to={`/projects/${projectId}/sdk-endpoints`}
              className="inline-block bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
            >
              Create an SDK endpoint
            </Link>
          }
        />
      ) : (
        <>
          {/* ── Headline verdict — the "find the main issue" payoff ─────────── */}
          <div className={`rounded-lg border ${overallStyle.border} ${overallStyle.bg} p-5 md:p-6 mb-6`}>
            <div className={`text-[11px] uppercase tracking-wider font-medium ${overallStyle.text} mb-2`}>
              {overallStyle.label}
            </div>
            <p className={`text-lg md:text-2xl font-bold leading-snug ${overallStyle.text}`}>
              {report!.overall_main_issue}
            </p>
            <div className="mt-4 max-w-2xl">
              <SplitBar
                networkMs={report!.overall_median_network_ms}
                serverMs={report!.overall_median_server_ms}
              />
              <div className="flex flex-wrap gap-x-6 gap-y-1 mt-3 text-xs text-gray-400 font-mono">
                <span>median wall {fmtMs(report!.overall_median_wall_ms)}</span>
                <span>server ratio {fmtRatio(report!.overall_server_ratio)}</span>
                {report!.split_anomaly_count > 0 && (
                  <span className="text-yellow-400" title="Attempts where reported server time exceeded observed wall time — clock skew or SDK span mismatch.">
                    ⚠ {report!.split_anomaly_count} split anomal{report!.split_anomaly_count === 1 ? 'y' : 'ies'}
                  </span>
                )}
              </div>
            </div>
          </div>

          {/* ── Legend ──────────────────────────────────────────────────────── */}
          <div className="flex items-center gap-4 mb-3 text-xs text-gray-500">
            <span className="flex items-center gap-1.5">
              <span className="inline-block w-3 h-3 rounded-sm" style={{ backgroundColor: NETWORK_COLOR }} /> Network
            </span>
            <span className="flex items-center gap-1.5">
              <span className="inline-block w-3 h-3 rounded-sm" style={{ backgroundColor: SERVER_COLOR }} /> Server
            </span>
          </div>

          {/* ── Per-endpoint table ──────────────────────────────────────────── */}
          <div className="table-container mb-6">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="px-4 py-2.5 text-left font-medium">Endpoint</th>
                  <th className="px-4 py-2.5 text-left font-medium">Verdict</th>
                  <th className="px-4 py-2.5 text-left font-medium w-64">Split (median)</th>
                  <th className="px-4 py-2.5 text-right font-medium">Server med / p95</th>
                  <th className="px-4 py-2.5 text-right font-medium">Network med / p95</th>
                  <th className="px-4 py-2.5 text-right font-medium">Server ratio</th>
                  <th className="px-4 py-2.5 text-right font-medium">Runs / attempts</th>
                  <th className="px-4 py-2.5 text-right font-medium">Anomalies</th>
                </tr>
              </thead>
              <tbody>
                {report!.groups.map((g: AppNetworkGroup) => (
                  <tr key={g.config_id} className="border-b border-gray-800/50 hover:bg-gray-800/20 align-top">
                    <td className="px-4 py-3 text-gray-200">{g.config_name}</td>
                    <td className="px-4 py-3"><VerdictBadge verdict={g.verdict} /></td>
                    <td className="px-4 py-3">
                      <SplitBar
                        networkMs={g.median_network_ms}
                        serverMs={g.median_server_ms}
                        p95NetworkMs={g.p95_network_ms}
                        p95ServerMs={g.p95_server_ms}
                      />
                    </td>
                    <td className="px-4 py-3 text-right font-mono text-xs text-purple-300">
                      {fmtMs(g.median_server_ms)} <span className="text-gray-600">/ {fmtMs(g.p95_server_ms)}</span>
                    </td>
                    <td className="px-4 py-3 text-right font-mono text-xs text-cyan-300">
                      {fmtMs(g.median_network_ms)} <span className="text-gray-600">/ {fmtMs(g.p95_network_ms)}</span>
                    </td>
                    <td className="px-4 py-3 text-right font-mono text-xs text-gray-300">{fmtRatio(g.server_ratio)}</td>
                    <td className="px-4 py-3 text-right font-mono text-xs text-gray-400">
                      {g.run_count} / {g.attempt_count}
                    </td>
                    <td className="px-4 py-3 text-right text-xs">
                      {g.split_anomaly_count > 0 ? (
                        <span
                          className="text-yellow-400"
                          title="Clock/instrumentation anomalies — reported server time exceeded observed wall time."
                        >
                          ⚠ {g.split_anomaly_count}
                        </span>
                      ) : (
                        <span className="text-gray-600">0</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* ── Formulas / disclaimer (verbatim from the response) ──────────── */}
          <div className="text-xs text-gray-600 space-y-1">
            <p className="font-mono">{report!.formulas.server_ms}</p>
            <p className="font-mono">{report!.formulas.network_ms}</p>
            <p className="font-mono">{report!.formulas.split}</p>
            <p className="font-mono">{report!.formulas.split_anomaly}</p>
            <p className="pt-1">
              Generated {new Date(report!.generated_at).toLocaleString()} · mode{' '}
              <span className="font-mono text-gray-500">{report!.mode}</span>
            </p>
          </div>
        </>
      )}
    </div>
  );
}
