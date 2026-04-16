import { useState, useCallback, useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { ComparisonReport, BenchmarkCaseComparison } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';

export function RunComparePage() {
  const { projectId } = useProject();
  const [searchParams] = useSearchParams();
  const idsParam = searchParams.get('ids') || '';
  const runIds = useMemo(() => idsParam.split(',').filter(Boolean), [idsParam]);
  const [report, setReport] = useState<ComparisonReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  usePageTitle('Compare Runs');

  const loadComparison = useCallback(() => {
    if (runIds.length < 2) return;
    api.compareTestRuns(runIds)
      .then(r => { setReport(r); setError(null); setLoading(false); })
      .catch(e => { setError(String(e)); setLoading(false); });
  }, [runIds]);

  usePolling(loadComparison, 0, runIds.length >= 2);

  // Handle invalid input without useEffect setState
  const tooFewIds = runIds.length < 2;

  if (tooFewIds) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Compare' }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Invalid input</h3>
          <p className="text-red-300 text-sm">At least two run IDs are required (pass ?ids=a,b,...)</p>
        </div>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Compare' }]} />
        <div className="text-gray-500 motion-safe:animate-pulse text-sm">Loading comparison...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="p-4 md:p-6">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Compare' }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Comparison failed</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  if (!report) return null;

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Compare' }]} />
      <h2 className="text-xl font-bold text-gray-100 mb-2">Run Comparison</h2>
      <p className="text-xs text-gray-500 mb-6">{runIds.length} runs compared</p>

      {report.cases.length === 0 ? (
        <div className="border border-gray-800 rounded p-8 text-center">
          <p className="text-gray-500 text-sm">No comparable cases found across these runs.</p>
        </div>
      ) : (
        <div className="table-container">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-500 bg-[var(--bg-surface)]">
                <th className="px-4 py-2.5 text-left font-medium">Case</th>
                <th className="px-4 py-2.5 text-left font-medium">Protocol</th>
                <th className="px-4 py-2.5 text-left font-medium">Metric</th>
                <th className="px-4 py-2.5 text-right font-medium">Baseline p50</th>
                {report.cases[0]?.candidates?.map((_, i) => (
                  <th key={i} className="px-4 py-2.5 text-right font-medium">
                    Run {i + 2} Delta
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {report.cases.map((c: BenchmarkCaseComparison) => (
                <tr key={c.case_id} className="border-b border-gray-800/30 hover:bg-gray-800/10">
                  <td className="px-4 py-2 text-gray-300 font-mono">{c.case_id.slice(0, 8)}</td>
                  <td className="px-4 py-2 text-gray-400">{c.protocol}</td>
                  <td className="px-4 py-2 text-gray-400">{c.metric_name} ({c.metric_unit})</td>
                  <td className="px-4 py-2 text-gray-200 text-right font-mono">
                    {c.baseline?.distribution?.median?.toFixed(2) ?? '-'}
                  </td>
                  {c.candidates?.map((cand, i) => {
                    const delta = cand.percent_delta;
                    const color = delta == null ? 'text-gray-600' :
                      (c.higher_is_better ? delta > 0 : delta < 0) ? 'text-green-400' : 'text-red-400';
                    return (
                      <td key={i} className={`px-4 py-2 text-right font-mono ${color}`}>
                        {delta != null ? `${delta > 0 ? '+' : ''}${delta.toFixed(1)}%` : '-'}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
