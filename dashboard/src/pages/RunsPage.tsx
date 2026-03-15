import { useState, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api, type RunSummary } from '../api/client';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';

export function RunsPage() {
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [targetSearch, setTargetSearch] = useState('');

  usePageTitle('Runs');

  const loadRuns = useCallback(() => {
    const params: { target_host?: string; limit?: number } = { limit: 50 };
    if (targetSearch.trim()) params.target_host = targetSearch.trim();
    api
      .getRuns(params)
      .then((data) => {
        setRuns(data);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [targetSearch]);

  usePolling(loadRuns, 15000);

  if (loading && runs.length === 0) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Test Runs</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading runs...</div>
      </div>
    );
  }

  if (error && runs.length === 0) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Test Runs</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load runs</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Test Runs</h2>
        <div>
          <label htmlFor="runs-target-search" className="sr-only">
            Search by target host
          </label>
          <input
            id="runs-target-search"
            type="search"
            value={targetSearch}
            onChange={(e) => setTargetSearch(e.target.value)}
            placeholder="Filter by target host..."
            className="bg-[#0a0b0f] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
        </div>
      </div>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh: {error}
        </div>
      )}

      <div className="bg-[#12131a] border border-gray-800 rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800 text-gray-500 text-xs">
              <th className="px-4 py-3 text-left">Run ID</th>
              <th className="px-4 py-3 text-left">Target</th>
              <th className="px-4 py-3 text-left">Modes</th>
              <th className="px-4 py-3 text-left">Success</th>
              <th className="px-4 py-3 text-left">Failed</th>
              <th className="px-4 py-3 text-left">Started</th>
            </tr>
          </thead>
          <tbody>
            {runs.map((run) => (
              <tr
                key={run.run_id}
                className="border-b border-gray-800/50 hover:bg-gray-800/20"
              >
                <td className="px-4 py-3">
                  <Link
                    to={`/runs/${run.run_id}`}
                    className="text-cyan-400 hover:underline font-mono text-xs"
                  >
                    {run.run_id.slice(0, 8)}
                  </Link>
                </td>
                <td className="px-4 py-3 text-gray-300">{run.target_host}</td>
                <td className="px-4 py-3 text-gray-500 text-xs">{run.modes}</td>
                <td className="px-4 py-3 text-green-400">{run.success_count}</td>
                <td className="px-4 py-3 text-red-400">
                  {run.failure_count > 0 ? run.failure_count : '-'}
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs">
                  {new Date(run.started_at).toLocaleString()}
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        {runs.length === 0 && (
          <p className="p-4 text-gray-600 text-sm text-center">
            No test runs stored yet. Complete a job to see results here.
          </p>
        )}
      </div>
    </div>
  );
}
