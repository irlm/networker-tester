import { useState, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api, type Job } from '../api/client';
import { StatusBadge } from '../components/common/StatusBadge';
import { CreateJobDialog } from '../components/CreateJobDialog';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';

const STATUS_OPTIONS = ['all', 'pending', 'running', 'completed', 'failed', 'cancelled'] as const;

function formatDuration(start: Date, end: Date): string {
  const ms = end.getTime() - start.getTime();
  if (ms < 1000) return `${ms}ms`;
  const secs = Math.floor(ms / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  const remainSecs = secs % 60;
  return `${mins}m ${remainSecs}s`;
}

export function JobsPage() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<string>('all');

  usePageTitle('Jobs');

  const loadJobs = useCallback(() => {
    const params: { status?: string; limit?: number } = { limit: 50 };
    if (statusFilter !== 'all') params.status = statusFilter;
    api
      .getJobs(params)
      .then((data) => {
        setJobs(data);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [statusFilter]);

  usePolling(loadJobs, 5000);

  if (loading && jobs.length === 0) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Jobs</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading jobs...</div>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Jobs</h2>
        <div className="flex items-center gap-3">
          <label htmlFor="jobs-status-filter" className="sr-only">
            Filter by status
          </label>
          <select
            id="jobs-status-filter"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            className="bg-[#0a0b0f] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
          >
            {STATUS_OPTIONS.map((s) => (
              <option key={s} value={s}>
                {s === 'all' ? 'All statuses' : s.charAt(0).toUpperCase() + s.slice(1)}
              </option>
            ))}
          </select>
          <button
            onClick={() => setShowCreate(true)}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
          >
            New Job
          </button>
        </div>
      </div>

      {showCreate && (
        <CreateJobDialog
          onClose={() => setShowCreate(false)}
          onCreated={loadJobs}
        />
      )}

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh: {error}
        </div>
      )}

      <div className="bg-[#12131a] border border-gray-800 rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800 text-gray-500 text-xs">
              <th className="px-4 py-3 text-left">Job ID</th>
              <th className="px-4 py-3 text-left">Target</th>
              <th className="px-4 py-3 text-left">Modes</th>
              <th className="px-4 py-3 text-left">Runs</th>
              <th className="px-4 py-3 text-left">Status</th>
              <th className="px-4 py-3 text-left">Duration</th>
              <th className="px-4 py-3 text-left">Created</th>
            </tr>
          </thead>
          <tbody>
            {jobs.map((job) => {
              const isActive = job.status === 'running' || job.status === 'assigned';
              const duration = job.started_at
                ? formatDuration(
                    new Date(job.started_at),
                    job.finished_at ? new Date(job.finished_at) : new Date()
                  )
                : '-';
              return (
                <tr
                  key={job.job_id}
                  className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${
                    isActive ? 'bg-cyan-500/5' : ''
                  }`}
                >
                  <td className="px-4 py-3">
                    <Link
                      to={`/jobs/${job.job_id}`}
                      className="text-cyan-400 hover:underline font-mono text-xs"
                    >
                      {job.job_id.slice(0, 8)}
                    </Link>
                  </td>
                  <td className="px-4 py-3 text-gray-300 text-xs max-w-48 truncate" title={job.config?.target}>
                    {job.config?.target}
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs max-w-32 truncate">
                    {job.config?.modes?.join(', ')}
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs">
                    {job.config?.runs ?? '-'}
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <StatusBadge status={job.status} />
                      {isActive && (
                        <span className="w-1.5 h-1.5 rounded-full bg-cyan-400 motion-safe:animate-pulse" />
                      )}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs font-mono">
                    {duration}
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs">
                    {new Date(job.created_at).toLocaleString()}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>

        {jobs.length === 0 && (
          <p className="p-4 text-gray-600 text-sm text-center">
            No jobs yet. Create one to get started.
          </p>
        )}
      </div>
    </div>
  );
}
