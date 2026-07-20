import { useState, useCallback, useMemo } from 'react';
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  CartesianGrid,
} from 'recharts';
import { api } from '../api/client';
import type { PerfPerCostReport, PerfPerCostGroup, PerfPerCostFamily } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { TOOLTIP_STYLE } from '../lib/chart';

const FAMILY_LABEL: Record<string, string> = {
  net: 'Network',
  http: 'HTTP',
  page: 'Page Load',
  thru: 'Throughput',
};

/** One flattened row: a tester group's aggregate for the selected family. */
interface Row {
  group: PerfPerCostGroup;
  fam: PerfPerCostFamily;
}

function fmtMetric(fam: PerfPerCostFamily, value: number | null): string {
  if (value === null) return '—';
  return fam.metric_label === 'throughput_mbps'
    ? `${value.toFixed(1)} Mbps`
    : `${value.toFixed(1)}ms`;
}

function fmtValueScore(fam: PerfPerCostFamily): string {
  if (fam.value_score === null) return '—';
  return fam.value_metric === 'mbps_per_dollar_hour'
    ? Math.round(fam.value_score).toLocaleString()
    : fam.value_score.toFixed(3);
}

/** Sort best-first: latency index ascending, Mbps/$ descending; unpriced last. */
function sortRows(rows: Row[]): Row[] {
  return [...rows].sort((a, b) => {
    if (a.fam.value_score === null) return b.fam.value_score === null ? 0 : 1;
    if (b.fam.value_score === null) return -1;
    return a.fam.value_metric === 'mbps_per_dollar_hour'
      ? b.fam.value_score - a.fam.value_score
      : a.fam.value_score - b.fam.value_score;
  });
}

export function ValueReportPage() {
  const { projectId } = useProject();
  const [report, setReport] = useState<PerfPerCostReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [family, setFamily] = useState<string | null>(null);

  usePageTitle('Value');

  const refresh = useCallback(() => {
    if (!projectId) return;
    api.getPerfPerCostReport(projectId)
      .then(r => { setReport(r); setLoading(false); })
      .catch(() => setLoading(false));
  }, [projectId]);

  usePolling(refresh, 60000);

  // Families present in the data, ordered by total run count (busiest first).
  const families = useMemo(() => {
    if (!report) return [];
    const counts = new Map<string, number>();
    for (const g of report.groups) {
      for (const f of g.families) {
        counts.set(f.family, (counts.get(f.family) ?? 0) + f.run_count);
      }
    }
    return [...counts.entries()].sort((a, b) => b[1] - a[1]).map(([id]) => id);
  }, [report]);

  const activeFamily = family ?? families[0] ?? null;

  const rows = useMemo(() => {
    if (!report || !activeFamily) return [];
    const out: Row[] = [];
    for (const group of report.groups) {
      const fam = group.families.find(f => f.family === activeFamily);
      if (fam) out.push({ group, fam });
    }
    return sortRows(out);
  }, [report, activeFamily]);

  const chartData = useMemo(() =>
    rows
      .filter(r => r.fam.value_score !== null)
      .map(r => ({
        name: `${r.group.provider} ${r.group.vm_size}`,
        value: r.fam.value_score,
      })),
  [rows]);

  const higherIsBetter = rows[0]?.fam.value_metric === 'mbps_per_dollar_hour';

  if (loading && !report) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Value</h2>
        <div className="space-y-3">
          {[1, 2, 3].map(i => (
            <div key={i} className="border border-gray-800 rounded p-4">
              <div className="h-4 w-48 rounded bg-gray-800/60 motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  const providers = report?.providers_with_data ?? 0;

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Value"
        subtitle={report
          ? `Provider performance per cost — ${report.completed_runs} completed runs across ${providers} provider${providers === 1 ? '' : 's'}`
          : undefined}
      />

      {!report || report.groups.length === 0 ? (
        <EmptyState
          message="No probe data to price yet"
          detail="This report compares providers once completed runs have persisted probe results. Run network tests or benchmarks on testers from at least two providers to see performance-per-dollar."
        />
      ) : providers < 2 ? (
        <EmptyState
          message="Need testers on at least two providers to compare value"
          detail={`Only ${report.groups[0].provider} has completed-run data so far. Provision a tester on a second provider (Azure / AWS / GCP) and run the same tests to unlock the comparison.`}
        />
      ) : (
        <>
          {report.missing_cost_skus.length > 0 && (
            <p className="text-xs text-yellow-400 bg-yellow-500/10 border border-yellow-500/20 rounded px-3 py-2 mb-4">
              {report.missing_cost_skus.length} tester group{report.missing_cost_skus.length === 1 ? ' has' : 's have'} no
              price row in the cost table ({report.missing_cost_skus.map(m => `${m.provider}/${m.vm_size}`).join(', ')})
              {' — '}shown with cost {'—'} and no value score, never dropped.
            </p>
          )}

          {/* Family selector */}
          <div className="flex flex-wrap gap-2 mb-4">
            {families.map(f => (
              <button
                key={f}
                onClick={() => setFamily(f)}
                className={`text-xs px-3 py-1.5 rounded border transition-colors ${
                  f === activeFamily
                    ? 'border-cyan-500/50 text-cyan-400 bg-cyan-500/10'
                    : 'border-gray-800 text-gray-500 hover:text-gray-300'
                }`}
              >
                {FAMILY_LABEL[f] ?? f}
              </button>
            ))}
          </div>

          {/* Comparison table */}
          <div className="table-container mb-6">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="px-4 py-2.5 text-left font-medium">Provider</th>
                  <th className="px-4 py-2.5 text-left font-medium">VM size</th>
                  <th className="px-4 py-2.5 text-left font-medium">Region</th>
                  <th className="px-4 py-2.5 text-right font-medium">Runs</th>
                  <th className="px-4 py-2.5 text-right font-medium">Samples</th>
                  <th className="px-4 py-2.5 text-right font-medium">Median</th>
                  <th className="px-4 py-2.5 text-right font-medium">p95</th>
                  <th className="px-4 py-2.5 text-right font-medium">$/hr</th>
                  <th className="px-4 py-2.5 text-right font-medium">
                    {higherIsBetter ? 'Mbps per $·hr' : 'p95 × $/hr'}
                    <span className="text-gray-600 font-normal ml-1">
                      ({higherIsBetter ? 'higher' : 'lower'} is better)
                    </span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r, i) => (
                  <tr
                    key={`${r.group.provider}/${r.group.vm_size}/${r.group.region}`}
                    className="border-b border-gray-800/50 hover:bg-gray-800/20"
                  >
                    <td className="px-4 py-3 text-gray-200">{r.group.provider}</td>
                    <td className="px-4 py-3 text-gray-400 font-mono text-xs">{r.group.vm_size}</td>
                    <td className="px-4 py-3 text-gray-500 text-xs">{r.group.region}</td>
                    <td className="px-4 py-3 text-gray-400 text-xs text-right font-mono">{r.fam.run_count}</td>
                    <td className="px-4 py-3 text-gray-500 text-xs text-right font-mono">{r.fam.sample_count}</td>
                    <td className="px-4 py-3 text-gray-300 text-xs text-right font-mono">
                      {fmtMetric(r.fam, r.fam.median)}
                    </td>
                    <td className="px-4 py-3 text-gray-400 text-xs text-right font-mono">
                      {r.fam.p95_ms !== null ? `${r.fam.p95_ms.toFixed(1)}ms` : '—'}
                    </td>
                    <td className="px-4 py-3 text-gray-300 text-xs text-right font-mono">
                      {r.group.hourly_usd !== null ? `$${r.group.hourly_usd.toFixed(4)}` : '—'}
                      {r.group.cost_note && (
                        <span className="text-yellow-500/80 ml-1" title={r.group.cost_note}>*</span>
                      )}
                    </td>
                    <td className={`px-4 py-3 text-xs text-right font-mono ${
                      i === 0 && r.fam.value_score !== null ? 'text-cyan-400' : 'text-gray-300'
                    }`}>
                      {fmtValueScore(r.fam)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* Value bars */}
          {chartData.length > 1 && (
            <div className="border border-gray-800 rounded p-4 mb-6">
              <p className="text-xs text-gray-500 mb-2">
                {higherIsBetter
                  ? 'Sustained Mbps per dollar-hour (higher is better)'
                  : 'Dollar-weighted p95 latency (lower is better)'}
              </p>
              <ResponsiveContainer width="100%" height={260}>
                <BarChart data={chartData} margin={{ top: 8, right: 16, left: 0, bottom: 8 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="#1a1b25" />
                  <XAxis dataKey="name" tick={{ fill: '#6b7280', fontSize: 11 }} />
                  <YAxis tick={{ fill: '#6b7280', fontSize: 11 }} />
                  <Tooltip contentStyle={TOOLTIP_STYLE} cursor={{ fill: 'rgba(71,191,255,0.05)' }} />
                  <Bar dataKey="value" fill="#47bfff" />
                </BarChart>
              </ResponsiveContainer>
            </div>
          )}

          {/* Formulas + disclaimer */}
          <div className="text-xs text-gray-600 space-y-1">
            <p className="font-mono">{report.formulas.latency_cost_index}</p>
            <p className="font-mono">{report.formulas.mbps_per_dollar_hour}</p>
            <p>
              Prices: static curated table ({report.cost_table.source}), as of{' '}
              {report.cost_table.as_of}. {report.cost_table.disclaimer}
            </p>
          </div>
        </>
      )}
    </div>
  );
}
