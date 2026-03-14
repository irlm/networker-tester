import { useEffect, useState } from 'react';
import { api, type RunSummary } from '../api/client';

export function RunsPage() {
  const [runs, setRuns] = useState<RunSummary[]>([]);

  useEffect(() => {
    api.getRuns({ limit: 50 }).then(setRuns).catch(console.error);
  }, []);

  return (
    <div className="p-6">
      <h2 className="text-xl font-bold text-gray-100 mb-6">Test Runs</h2>

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
                <td className="px-4 py-3 font-mono text-xs text-cyan-400">
                  {run.run_id.slice(0, 8)}
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
