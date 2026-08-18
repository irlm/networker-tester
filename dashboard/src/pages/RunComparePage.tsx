import { useMemo } from 'react';
import { useSearchParams } from 'react-router';
import { errorMessage } from '../api/client';
import type { BenchmarkCaseComparison } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { ErrorState, LoadingState } from '../components/common/AsyncState';
import { PageShell } from '../components/common/PageShell';
import { useRunComparisonQuery } from '../features/runs/queries';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

export function RunComparePage() {
  const { projectId } = useProject();
  const [searchParams] = useSearchParams();
  const idsParam = searchParams.get('ids') || '';
  const runIds = useMemo(() => idsParam.split(',').filter(Boolean), [idsParam]);
  const comparison = useRunComparisonQuery(runIds);

  usePageTitle('Compare Runs');

  const tooFewIds = runIds.length < 2;
  const breadcrumb = <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Compare' }]} />;

  if (tooFewIds) {
    return (
      <PageShell before={breadcrumb} title="Run Comparison">
        <ErrorState
          title="Invalid comparison"
          message="Select at least two runs before opening a comparison."
        />
      </PageShell>
    );
  }

  if (comparison.isPending) {
    return (
      <PageShell before={breadcrumb} title="Run Comparison">
        <LoadingState label="Loading comparison…" />
      </PageShell>
    );
  }

  if (comparison.error) {
    return (
      <PageShell before={breadcrumb} title="Run Comparison">
        <ErrorState
          title="Comparison failed"
          message={errorMessage(comparison.error)}
          onRetry={() => { void comparison.refetch(); }}
        />
      </PageShell>
    );
  }

  const report = comparison.data;
  if (!report) return null;

  return (
    <PageShell before={breadcrumb} title="Run Comparison" subtitle={`${runIds.length} runs compared`}>
      {report.cases.length === 0 ? (
        <div className="border border-gray-800 rounded p-8 text-center">
          <p className="text-gray-400 text-sm">No comparable cases found across these runs.</p>
        </div>
      ) : (
        <div className="table-container">
          <table className="w-full text-xs">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-400 bg-[var(--bg-surface)]">
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
                  <td className="px-4 py-2 text-gray-300">{c.case_id.slice(0, 8)}</td>
                  <td className="px-4 py-2 text-gray-400">{c.protocol}</td>
                  <td className="px-4 py-2 text-gray-400">{c.metric_name} ({c.metric_unit})</td>
                  <td className="px-4 py-2 text-gray-200 text-right">
                    {c.baseline?.distribution?.median?.toFixed(2) ?? '-'}
                  </td>
                  {c.candidates?.map((cand, i) => {
                    const delta = cand.percent_delta;
                    const color = delta == null ? 'text-faint' :
                      (c.higher_is_better ? delta > 0 : delta < 0) ? 'text-green-400' : 'text-red-400';
                    return (
                      <td key={i} className={`px-4 py-2 text-right ${color}`}>
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
    </PageShell>
  );
}
