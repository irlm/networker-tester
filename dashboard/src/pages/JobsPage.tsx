import { useState, useCallback } from 'react';
import { Link } from 'react-router-dom';
import { api, type Job, type Agent, type Deployment } from '../api/client';
import { StatusBadge } from '../components/common/StatusBadge';
import { CreateJobDialog } from '../components/CreateJobDialog';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';

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
  const [testers, setTesters] = useState<Agent[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [showAddTester, setShowAddTester] = useState(false);
  const [addTesterMode, setAddTesterMode] = useState<'local' | 'endpoint' | 'ssh'>('local');
  const [sshHost, setSshHost] = useState('');
  const [sshUser, setSshUser] = useState('root');
  const [sshPort, setSshPort] = useState(22);
  const [testerName, setTesterName] = useState('');
  const [selectedEndpoint, setSelectedEndpoint] = useState('');
  const [addingTester, setAddingTester] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState<string>('all');
  const [showTesters, setShowTesters] = useState(false);
  const addToast = useToast();

  usePageTitle('Tests');

  const loadJobs = useCallback(() => {
    const params: { status?: string; limit?: number } = { limit: 20 };
    if (statusFilter !== 'all') params.status = statusFilter;
    api.getJobs(params).then((data) => {
      setJobs(data);
      setError(null);
      setLoading(false);
    }).catch((e) => {
      setError(String(e));
      setLoading(false);
    });
  }, [statusFilter]);

  const loadTesters = useCallback(() => {
    api.getAgents().then(r => setTesters(r.agents)).catch(() => {});
    api.getDeployments({ limit: 20 }).then(deps => {
      setDeployments(deps.filter(d => d.status === 'completed' && d.endpoint_ips && d.endpoint_ips.length > 0));
    }).catch(() => {});
  }, []);

  usePolling(loadJobs, 5000);
  usePolling(loadTesters, 10000);

  const handleAddTester = async () => {
    setAddingTester(true);
    try {
      if (addTesterMode === 'local') {
        const result = await api.createAgent({
          name: testerName.trim() || 'local-tester',
          location: 'local',
        });
        addToast('success', `Tester "${result.name}" starting...`);
      } else if (addTesterMode === 'ssh') {
        if (!sshHost.trim()) { setAddingTester(false); return; }
        const result = await api.createAgent({
          name: testerName.trim() || `tester-${sshHost}`,
          location: 'ssh',
          ssh_host: sshHost,
          ssh_user: sshUser,
          ssh_port: sshPort,
        });
        addToast('success', `Tester "${result.name}" deploying via SSH...`);
      } else if (addTesterMode === 'endpoint') {
        if (!selectedEndpoint) { setAddingTester(false); return; }
        // Deploy tester to an existing endpoint machine
        const dep = deployments.find(d =>
          d.endpoint_ips?.includes(selectedEndpoint)
        );
        const result = await api.createAgent({
          name: testerName.trim() || `tester-${selectedEndpoint}`,
          location: 'ssh',
          ssh_host: selectedEndpoint,
          ssh_user: 'azureuser', // Default for Azure VMs
          ssh_port: 22,
          region: dep?.provider_summary || undefined,
        });
        addToast('success', `Tester "${result.name}" deploying to endpoint...`);
      }
      setShowAddTester(false);
      setTesterName('');
      setSshHost('');
      setSelectedEndpoint('');
      loadTesters();
    } catch {
      addToast('error', 'Failed to add tester');
    } finally {
      setAddingTester(false);
    }
  };

  const handleDeleteTester = async (id: string, name: string) => {
    try {
      await api.deleteAgent(id);
      addToast('info', `Tester "${name}" removed`);
      loadTesters();
    } catch {
      addToast('error', 'Failed to remove tester');
    }
  };

  if (loading && jobs.length === 0) {
    return (
      <div className="p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Tests</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading...</div>
      </div>
    );
  }

  const onlineTesters = testers.filter(t => t.status === 'online');

  return (
    <div className="p-6">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Tests</h2>
        <div className="flex items-center gap-3">
          <button
            onClick={() => setShowTesters(!showTesters)}
            className={`flex items-center gap-2 px-3 py-1.5 rounded text-xs border transition-colors ${
              showTesters
                ? 'border-cyan-500/30 bg-cyan-500/10 text-cyan-400'
                : 'border-gray-700 text-gray-400 hover:border-gray-600'
            }`}
          >
            <span className={`w-2 h-2 rounded-full ${onlineTesters.length > 0 ? 'bg-green-400' : 'bg-gray-500'}`} />
            {testers.length} tester{testers.length !== 1 ? 's' : ''} ({onlineTesters.length} online)
          </button>

          <select
            id="tests-status-filter"
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value)}
            aria-label="Filter by status"
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
            New Test
          </button>
        </div>
      </div>

      {showCreate && (
        <CreateJobDialog onClose={() => setShowCreate(false)} onCreated={loadJobs} />
      )}

      {/* Testers Panel */}
      {showTesters && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4 mb-6">
          <div className="flex items-center justify-between mb-3">
            <p className="text-sm text-gray-400 font-medium">Testers</p>
            <button
              onClick={() => setShowAddTester(!showAddTester)}
              className="text-xs text-cyan-400 hover:text-cyan-300"
            >
              {showAddTester ? 'Cancel' : '+ Add Tester'}
            </button>
          </div>

          {/* Add Tester Form */}
          {showAddTester && (
            <div className="bg-[#0a0b0f] border border-gray-800 rounded p-3 mb-3">
              <div className="flex gap-2 mb-3">
                {([
                  { id: 'local', label: 'Local (this machine)' },
                  { id: 'endpoint', label: 'On deployed endpoint' },
                  { id: 'ssh', label: 'Remote (SSH)' },
                ] as const).map(opt => (
                  <button
                    key={opt.id}
                    type="button"
                    onClick={() => setAddTesterMode(opt.id)}
                    className={`px-3 py-1 text-xs rounded border transition-colors ${
                      addTesterMode === opt.id
                        ? 'border-cyan-500/50 bg-cyan-500/10 text-cyan-400'
                        : 'border-gray-700 text-gray-400 hover:border-gray-600'
                    }`}
                  >
                    {opt.label}
                  </button>
                ))}
              </div>

              {addTesterMode === 'local' && (
                <p className="text-xs text-gray-500 mb-2">
                  Starts a tester on this machine. Can probe any reachable target.
                </p>
              )}

              {addTesterMode === 'endpoint' && (
                <div className="mb-2">
                  <p className="text-xs text-gray-500 mb-2">
                    Install a tester on an existing deployed endpoint. Tests from that network location.
                  </p>
                  <select
                    value={selectedEndpoint}
                    onChange={e => setSelectedEndpoint(e.target.value)}
                    className="w-full bg-[#12131a] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  >
                    <option value="">Select endpoint...</option>
                    {deployments.flatMap(d =>
                      (d.endpoint_ips || []).map(ip => (
                        <option key={ip} value={ip}>
                          {d.name} ({ip})
                        </option>
                      ))
                    )}
                  </select>
                </div>
              )}

              {addTesterMode === 'ssh' && (
                <div className="grid grid-cols-3 gap-2 mb-2">
                  <input
                    value={sshHost}
                    onChange={e => setSshHost(e.target.value)}
                    placeholder="Host / IP"
                    className="bg-[#12131a] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                  <input
                    value={sshUser}
                    onChange={e => setSshUser(e.target.value)}
                    placeholder="SSH user"
                    className="bg-[#12131a] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                  <input
                    type="number"
                    value={sshPort}
                    onChange={e => setSshPort(Number(e.target.value))}
                    placeholder="Port"
                    className="bg-[#12131a] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              )}

              <div className="flex gap-2 items-center">
                <input
                  value={testerName}
                  onChange={e => setTesterName(e.target.value)}
                  placeholder="Tester name (optional)"
                  className="flex-1 bg-[#12131a] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <button
                  onClick={handleAddTester}
                  disabled={addingTester || (addTesterMode === 'ssh' && !sshHost.trim()) || (addTesterMode === 'endpoint' && !selectedEndpoint)}
                  className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
                >
                  {addingTester ? 'Adding...' : addTesterMode === 'local' ? 'Start' : 'Deploy'}
                </button>
              </div>
            </div>
          )}

          {/* Tester List */}
          {testers.length === 0 ? (
            <p className="text-gray-600 text-sm">No testers registered. The dashboard auto-starts a local tester on first run.</p>
          ) : (
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
              {testers.map(t => (
                <div key={t.agent_id} className="bg-[#0a0b0f] border border-gray-800 rounded p-3">
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-sm text-gray-200">{t.name}</span>
                    <div className="flex items-center gap-2">
                      <StatusBadge status={t.status} />
                      <button
                        onClick={() => handleDeleteTester(t.agent_id, t.name)}
                        className="text-xs text-gray-600 hover:text-red-400"
                        title="Remove tester"
                      >
                        &#x2715;
                      </button>
                    </div>
                  </div>
                  <div className="text-xs text-gray-500">
                    {t.provider && <span>{t.provider} </span>}
                    {t.region && <span>{t.region} </span>}
                    {t.version && <span>v{t.version} </span>}
                    {t.last_heartbeat && (
                      <span>seen {new Date(t.last_heartbeat).toLocaleTimeString()}</span>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh: {error}
        </div>
      )}

      {/* Tests Table */}
      <div className="bg-[#12131a] border border-gray-800 rounded-lg overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800 text-gray-500 text-xs">
              <th className="px-4 py-3 text-left">Test ID</th>
              <th className="px-4 py-3 text-left">Target</th>
              <th className="px-4 py-3 text-left">Modes</th>
              <th className="px-4 py-3 text-left">Runs</th>
              <th className="px-4 py-3 text-left">Tester</th>
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
              const testerName = job.agent_id
                ? testers.find(t => t.agent_id === job.agent_id)?.name || job.agent_id.slice(0, 8)
                : '-';
              return (
                <tr
                  key={job.job_id}
                  className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${isActive ? 'bg-cyan-500/5' : ''}`}
                >
                  <td className="px-4 py-3">
                    <Link to={`/tests/${job.job_id}`} className="text-cyan-400 hover:underline font-mono text-xs">
                      {job.job_id.slice(0, 8)}
                    </Link>
                  </td>
                  <td className="px-4 py-3 text-gray-300 text-xs max-w-48 truncate" title={job.config?.target}>
                    {job.config?.target}
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs max-w-32 truncate">
                    {job.config?.modes?.join(', ')}
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs">{job.config?.runs ?? '-'}</td>
                  <td className="px-4 py-3 text-gray-500 text-xs">{testerName}</td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <StatusBadge status={job.status} />
                      {isActive && <span className="w-1.5 h-1.5 rounded-full bg-cyan-400 motion-safe:animate-pulse" />}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs font-mono">{duration}</td>
                  <td className="px-4 py-3 text-gray-500 text-xs">{new Date(job.created_at).toLocaleString()}</td>
                </tr>
              );
            })}
          </tbody>
        </table>

        {jobs.length === 0 && (
          <p className="p-4 text-gray-600 text-sm text-center">No tests yet. Create one to get started.</p>
        )}
      </div>
    </div>
  );
}
