import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type Job } from '../api/client';
import { StatusBadge } from '../components/common/StatusBadge';

export function JobsPage() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [showCreate, setShowCreate] = useState(false);

  const loadJobs = () => {
    api.getJobs({ limit: 50 }).then(setJobs).catch(console.error);
  };

  useEffect(() => {
    loadJobs();
    const interval = setInterval(loadJobs, 5000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Jobs</h2>
        <button
          onClick={() => setShowCreate(true)}
          className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors"
        >
          New Job
        </button>
      </div>

      {showCreate && (
        <CreateJobDialog
          onClose={() => setShowCreate(false)}
          onCreated={loadJobs}
        />
      )}

      <div className="bg-[#12131a] border border-gray-800 rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800 text-gray-500 text-xs">
              <th className="px-4 py-3 text-left">Job ID</th>
              <th className="px-4 py-3 text-left">Target</th>
              <th className="px-4 py-3 text-left">Modes</th>
              <th className="px-4 py-3 text-left">Status</th>
              <th className="px-4 py-3 text-left">Created</th>
            </tr>
          </thead>
          <tbody>
            {jobs.map((job) => (
              <tr
                key={job.job_id}
                className="border-b border-gray-800/50 hover:bg-gray-800/20"
              >
                <td className="px-4 py-3">
                  <Link
                    to={`/jobs/${job.job_id}`}
                    className="text-cyan-400 hover:underline font-mono text-xs"
                  >
                    {job.job_id.slice(0, 8)}
                  </Link>
                </td>
                <td className="px-4 py-3 text-gray-300">{job.config?.target}</td>
                <td className="px-4 py-3 text-gray-500 text-xs">
                  {job.config?.modes?.join(', ')}
                </td>
                <td className="px-4 py-3">
                  <StatusBadge status={job.status} />
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs">
                  {new Date(job.created_at).toLocaleString()}
                </td>
              </tr>
            ))}
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

function CreateJobDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [target, setTarget] = useState('https://localhost:8443/health');
  const [modes, setModes] = useState('http1,http2');
  const [runs, setRuns] = useState(3);
  const [insecure, setInsecure] = useState(true);
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    try {
      await api.createJob({
        target,
        modes: modes.split(',').map((m) => m.trim()),
        runs,
        concurrency: 1,
        timeout_secs: 30,
        payload_sizes: [],
        insecure,
        dns_enabled: true,
        connection_reuse: false,
      });
      onCreated();
      onClose();
    } catch (err) {
      console.error('Failed to create job:', err);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <form
        onSubmit={handleSubmit}
        className="bg-[#12131a] border border-gray-800 rounded-lg p-6 w-[500px]"
      >
        <h3 className="text-lg font-bold text-gray-100 mb-4">New Test Job</h3>

        <label className="block text-xs text-gray-400 mb-1">Target URL</label>
        <input
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-3 focus:outline-none focus:border-cyan-500"
        />

        <label className="block text-xs text-gray-400 mb-1">
          Modes (comma-separated)
        </label>
        <input
          value={modes}
          onChange={(e) => setModes(e.target.value)}
          className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 mb-3 focus:outline-none focus:border-cyan-500"
        />

        <div className="flex gap-4 mb-3">
          <div className="flex-1">
            <label className="block text-xs text-gray-400 mb-1">Runs</label>
            <input
              type="number"
              value={runs}
              onChange={(e) => setRuns(Number(e.target.value))}
              className="w-full bg-[#0a0b0f] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
            />
          </div>
          <div className="flex items-end pb-1">
            <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
              <input
                type="checkbox"
                checked={insecure}
                onChange={(e) => setInsecure(e.target.checked)}
                className="accent-cyan-500"
              />
              Insecure (skip TLS verify)
            </label>
          </div>
        </div>

        <div className="flex justify-end gap-3 mt-4">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={loading}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
          >
            {loading ? 'Creating...' : 'Create Job'}
          </button>
        </div>
      </form>
    </div>
  );
}
