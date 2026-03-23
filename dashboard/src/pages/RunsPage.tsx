import { useState, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api, type RunSummary } from '../api/client';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

export function RunsPage() {
  const { projectId } = useProject();
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [targetSearch, setTargetSearch] = useState('');

  usePageTitle('Runs');

  const loadRuns = useCallback(() => {
    if (!projectId) return;
    const params: { target_host?: string; limit?: number } = { limit: 50 };
    if (targetSearch.trim()) params.target_host = targetSearch.trim();
    api
      .getRuns(projectId, params)
      .then((data) => {
        setRuns(data);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [targetSearch, projectId]);

  usePolling(loadRuns, 15000);

  if (loading && runs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Test Runs</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading test runs...</div>
      </div>
    );
  }

  if (error && runs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">Test Runs</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load runs</h3>
          <p className="text-red-300 text-sm">Could not fetch test runs. Check your connection and try refreshing.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Test Runs</h2>
        <div>
          <label htmlFor="runs-target-search" className="sr-only">
            Search by target host
          </label>
          <input
            id="runs-target-search"
            type="search"
            value={targetSearch}
            onChange={(e) => setTargetSearch(e.target.value)}
            placeholder="Filter by host..."
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-40 md:w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
        </div>
      </div>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh runs. Retrying automatically.
        </div>
      )}

      {/* ── Mobile card layout (< md) ── */}
      <div className="md:hidden space-y-2">
        {runs.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">No test runs yet</p>
            <p className="text-gray-700 text-xs mt-1">Runs appear here after a test completes. Start one from the Tests page.</p>
          </div>
        ) : runs.map((run) => (
          <Link
            key={run.run_id}
            to={`/projects/${projectId}/runs/${run.run_id}`}
            className="block border border-gray-800 rounded p-3"
          >
            <div className="flex items-center justify-between mb-1">
              <span className="text-cyan-400 font-mono text-xs">{run.run_id.slice(0, 8)}</span>
              <div className="flex items-center gap-2 text-xs">
                <span className="text-green-400">{run.success_count} ok</span>
                {run.failure_count > 0 && (
                  <span className="text-red-400">{run.failure_count} fail</span>
                )}
              </div>
            </div>
            <p className="text-gray-300 text-xs truncate mb-1">{run.target_host}</p>
            <div className="flex items-center gap-3 text-xs text-gray-500">
              <span>{run.modes}</span>
              <span>{new Date(run.started_at).toLocaleTimeString()}</span>
            </div>
          </Link>
        ))}
      </div>

      {/* ── Desktop/iPad table (≥ md) ── */}
      <div className="hidden md:block table-container">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Run ID</th>
              <th className="px-4 py-2.5 text-left font-medium">Target</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Modes</th>
              <th className="px-4 py-2.5 text-left font-medium">Success</th>
              <th className="px-4 py-2.5 text-left font-medium">Failed</th>
              <th className="px-4 py-2.5 text-left font-medium">Started</th>
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
                    to={`/projects/${projectId}/runs/${run.run_id}`}
                    className="text-cyan-400 hover:underline font-mono text-xs"
                  >
                    {run.run_id.slice(0, 8)}
                  </Link>
                </td>
                <td className="px-4 py-3 text-gray-300 text-xs truncate max-w-48">{run.target_host}</td>
                <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">{run.modes}</td>
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
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">No test runs yet</p>
            <p className="text-gray-700 text-xs mt-1">Runs appear here after a test completes. Start one from the Tests page.</p>
          </div>
        )}
      </div>
    </div>
  );
}
