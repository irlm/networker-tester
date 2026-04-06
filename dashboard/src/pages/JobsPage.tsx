import { useState, useCallback, useMemo, useEffect, useRef } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api, type Job, type Agent, type Deployment } from '../api/client';
import type { ProjectMember } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { Combobox, type ComboboxOption } from '../components/common/Combobox';
import { FilterBar, FilterChip, ScopeChip } from '../components/common/FilterBar';
import { CreateJobDialog } from '../components/CreateJobDialog';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useRenderLog } from '../hooks/useRenderLog';
import { useToast } from '../hooks/useToast';
import { formatDuration } from '../lib/format';
import { stableSet } from '../lib/stableUpdate';
import { useProject } from '../hooks/useProject';

const STATUS_OPTIONS = ['all', 'pending', 'running', 'completed', 'failed', 'cancelled'] as const;
const TYPE_OPTIONS = ['all', 'test', 'tls'] as const;

function isTlsProfileJob(job: Job) {
  return Boolean(job.config?.tls_profile_url);
}

function jobLabel(job: Job) {
  return isTlsProfileJob(job) ? 'TLS Profile' : 'Test';
}

function jobTarget(job: Job) {
  return job.config?.tls_profile_url || job.config?.target || '-';
}

function jobSummary(job: Job) {
  if (!isTlsProfileJob(job)) return job.config?.modes?.join(', ') || '-';
  const bits: string[] = [job.config?.tls_profile_target_kind || 'external-url'];
  if (job.config?.tls_profile_ip) bits.push('IP override');
  if (job.config?.tls_profile_sni) bits.push('SNI override');
  return bits.join(' · ');
}

function jobResultLink(projectId: string, job: Job) {
  if (job.tls_profile_run_id) return `/projects/${projectId}/tls-profiles/${job.tls_profile_run_id}`;
  if (job.run_id) return `/projects/${projectId}/runs/${job.run_id}`;
  return null;
}

export function JobsPage() {
  const { projectId, isOperator } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [jobs, setJobs] = useState<Job[]>([]);
  const [testers, setTesters] = useState<Agent[]>([]);
  const [deployments, setDeployments] = useState<Deployment[]>([]);
  const [members, setMembers] = useState<ProjectMember[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [showAddTester, setShowAddTester] = useState(false);
  const [addTesterMode, setAddTesterMode] = useState<'cloud' | 'endpoint' | 'ssh'>('cloud');
  const [sshHost, setSshHost] = useState('');
  const [sshUser, setSshUser] = useState('root');
  const [sshPort, setSshPort] = useState(22);
  const [testerName, setTesterName] = useState('');
  const [selectedEndpoint, setSelectedEndpoint] = useState('');
  const [addingTester, setAddingTester] = useState(false);
  const [cloudRegion, setCloudRegion] = useState('eastus');
  const [cloudVmSize, setCloudVmSize] = useState('Standard_B1s');
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showTesters, setShowTesters] = useState(false);
  const addToast = useToast();
  const jobsFingerprint = useRef('');
  const testersFingerprint = useRef('');

  // Filters — persisted to URL search params
  const statusFilter = searchParams.get('status') || 'all';
  const agentFilter = searchParams.get('agent') || '';
  const createdByFilter = searchParams.get('created_by') || '';
  const typeFilter = searchParams.get('type') || 'all';
  const markRender = useRenderLog('JobsPage');

  const setFilter = useCallback((key: string, value: string) => {
    markRender(`filter:${key}`);
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (!value || value === 'all') {
        next.delete(key);
      } else {
        next.set(key, value);
      }
      return next;
    }, { replace: true });
  }, [setSearchParams, markRender]);

  const clearAllFilters = useCallback(() => {
    setSearchParams({}, { replace: true });
  }, [setSearchParams]);

  usePageTitle('Tests');

  const loadJobs = useCallback(() => {
    if (!projectId) return;
    const params: { status?: string; agent_id?: string; created_by?: string; limit?: number } = { limit: 20 };
    if (statusFilter !== 'all') params.status = statusFilter;
    if (agentFilter) params.agent_id = agentFilter;
    if (createdByFilter) params.created_by = createdByFilter;
    api.getJobs(projectId, params).then((data) => {
      const changed = stableSet(setJobs, data, jobsFingerprint);
      if (changed) markRender('api:jobs', data.length);
      setError(null);
      setLoading(false);
    }).catch((e) => {
      setError(String(e));
      setLoading(false);
    });
  }, [statusFilter, agentFilter, createdByFilter, projectId, markRender]);

  const loadTesters = useCallback(() => {
    if (!projectId) return;
    api.getAgents(projectId).then(r => stableSet(setTesters, r.agents, testersFingerprint)).catch(() => {});
    api.getDeployments(projectId, { limit: 20 }).then(deps => {
      setDeployments(deps.filter(d => d.status === 'completed' && d.endpoint_ips && d.endpoint_ips.length > 0));
    }).catch(() => {});
  }, [projectId]);

  // Load project members for "created by" filter (operator+ only)
  useEffect(() => {
    if (!projectId || !isOperator) return;
    api.getProjectMembers(projectId).then(setMembers).catch(() => {});
  }, [projectId, isOperator]);

  usePolling(loadJobs, 5000);
  usePolling(loadTesters, 10000);

  // Build combobox options + tester name lookup map
  const testerMap = useMemo(() =>
    new Map(testers.map(t => [t.agent_id, t.name])),
    [testers],
  );

  const agentOptions: ComboboxOption[] = useMemo(() =>
    testers.map(t => ({
      value: t.agent_id,
      label: t.name,
      detail: [t.region, t.status].filter(Boolean).join(' · '),
      group: t.status === 'online' ? 'Online' : 'Offline',
    })),
    [testers],
  );

  const memberOptions: ComboboxOption[] = useMemo(() =>
    members.map(m => ({
      value: m.user_id,
      label: m.display_name || m.email,
      detail: m.role,
    })),
    [members],
  );

  // Client-side type filter (test vs tls)
  const filteredJobs = useMemo(() => {
    if (typeFilter === 'all') return jobs;
    return jobs.filter(j => typeFilter === 'tls' ? isTlsProfileJob(j) : !isTlsProfileJob(j));
  }, [jobs, typeFilter]);

  // Active filter count for the chip bar
  const activeFilterCount = [
    statusFilter !== 'all',
    agentFilter,
    createdByFilter,
    typeFilter !== 'all',
  ].filter(Boolean).length;

  const handleAddTester = async () => {
    setAddingTester(true);
    try {
      if (addTesterMode === 'cloud') {
        const vmName = testerName.trim() || `tester-${cloudRegion}-${Date.now().toString(36).slice(-4)}`;
        await api.deployTesterVm(projectId, {
          name: vmName,
          provider: 'azure',
          region: cloudRegion,
          vm_size: cloudVmSize,
        });
        addToast('success', `Tester VM "${vmName}" deploying... (~3 minutes)`);
      } else if (addTesterMode === 'ssh') {
        if (!sshHost.trim()) { setAddingTester(false); return; }
        const result = await api.createAgent(projectId, {
          name: testerName.trim() || `tester-${sshHost}`,
          location: 'ssh',
          ssh_host: sshHost,
          ssh_user: sshUser,
          ssh_port: sshPort,
        });
        addToast('success', `Tester "${result.name}" deploying via SSH...`);
      } else if (addTesterMode === 'endpoint') {
        if (!selectedEndpoint) { setAddingTester(false); return; }
        const dep = deployments.find(d =>
          d.endpoint_ips?.includes(selectedEndpoint)
        );
        const result = await api.createAgent(projectId, {
          name: testerName.trim() || `tester-${selectedEndpoint}`,
          location: 'ssh',
          ssh_host: selectedEndpoint,
          ssh_user: 'azureuser',
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
      await api.deleteAgent(projectId, id);
      addToast('info', `Tester "${name}" removed`);
      loadTesters();
    } catch {
      addToast('error', 'Failed to remove tester');
    }
  };

  if (loading && jobs.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <div className="flex items-center justify-between mb-6">
          <h2 className="text-lg md:text-xl font-bold text-gray-100">Tests</h2>
          <div className="h-7 w-20 rounded bg-gray-800 motion-safe:animate-pulse" />
        </div>
        <div className="space-y-3 md:hidden">
          {[1, 2, 3, 4].map(i => (
            <div key={i} className="border border-gray-800 rounded p-4 space-y-2">
              <div className="h-4 w-32 rounded bg-gray-800/60 motion-safe:animate-pulse" />
              <div className="h-3 w-48 rounded bg-gray-800/40 motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
        <div className="hidden md:block table-container">
          <div className="bg-[var(--bg-surface)] px-4 py-2.5 border-b border-gray-800/50">
            <div className="flex gap-8">
              {[48, 80, 56, 32, 56, 48, 56, 72].map((w, i) => (
                <div key={i} className="h-3 rounded bg-gray-800/60 motion-safe:animate-pulse" style={{ width: w }} />
              ))}
            </div>
          </div>
          {[1, 2, 3, 4].map(i => (
            <div key={i} className="px-4 py-3 border-b border-gray-800/30 flex gap-8">
              {[48, 96, 64, 24, 56, 48, 40, 80].map((w, j) => (
                <div key={j} className="h-3 rounded bg-gray-800/40 motion-safe:animate-pulse" style={{ width: w }} />
              ))}
            </div>
          ))}
        </div>
      </div>
    );
  }

  const onlineTesters = testers.filter(t => t.status === 'online');

  // Resolve names for filter chips
  const agentName = agentFilter ? (testers.find(t => t.agent_id === agentFilter)?.name || agentFilter.slice(0, 8)) : '';
  const memberName = createdByFilter ? (members.find(m => m.user_id === createdByFilter)?.display_name || members.find(m => m.user_id === createdByFilter)?.email || createdByFilter.slice(0, 8)) : '';

  return (
    <div className="p-4 md:p-6">
      {/* Header */}
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2 flex-wrap">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Tests</h2>
        <div className="flex items-center gap-2 md:gap-3 flex-wrap">
          <button
            onClick={() => setShowTesters(!showTesters)}
            className={`flex items-center gap-2 px-3 py-1.5 rounded text-xs border transition-colors ${
              showTesters
                ? 'border-cyan-500/30 bg-cyan-500/10 text-cyan-400'
                : 'border-gray-700 text-gray-400 hover:border-gray-600'
            }`}
          >
            <span className={`w-2 h-2 rounded-full ${onlineTesters.length > 0 ? 'bg-green-400' : 'bg-gray-500'}`} />
            <span className="hidden sm:inline">{testers.length} tester{testers.length !== 1 ? 's' : ''}</span> ({onlineTesters.length} online)
          </button>
          {isOperator && (
            <button
              onClick={() => setShowCreate(true)}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-1.5 rounded text-sm transition-colors"
            >
              New Test
            </button>
          )}
        </div>
      </div>

      {/* ── Filter Bar ── */}
      <FilterBar
        activeCount={activeFilterCount}
        onClearAll={clearAllFilters}
        chips={
          <>
            {!isOperator && <ScopeChip label="Showing: your tests only" />}
            {statusFilter !== 'all' && (
              <FilterChip label="Status" value={statusFilter} onClear={() => setFilter('status', 'all')} />
            )}
            {agentFilter && (
              <FilterChip label="Tester" value={agentName} onClear={() => setFilter('agent', '')} />
            )}
            {createdByFilter && (
              <FilterChip label="Created by" value={memberName} onClear={() => setFilter('created_by', '')} />
            )}
            {typeFilter !== 'all' && (
              <FilterChip label="Type" value={typeFilter === 'tls' ? 'TLS Profile' : 'Test'} onClear={() => setFilter('type', 'all')} />
            )}
          </>
        }
      >
        {/* Status dropdown */}
        <select
          id="tests-status-filter"
          value={statusFilter}
          onChange={(e) => setFilter('status', e.target.value)}
          aria-label="Filter by status"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {STATUS_OPTIONS.map((s) => (
            <option key={s} value={s}>
              {s === 'all' ? 'All statuses' : s.charAt(0).toUpperCase() + s.slice(1)}
            </option>
          ))}
        </select>

        {/* Type dropdown */}
        <select
          value={typeFilter}
          onChange={(e) => setFilter('type', e.target.value)}
          aria-label="Filter by type"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {TYPE_OPTIONS.map(t => (
            <option key={t} value={t}>
              {t === 'all' ? 'All types' : t === 'tls' ? 'TLS Profile' : 'Test'}
            </option>
          ))}
        </select>

        {/* Agent/Tester combobox */}
        <Combobox
          value={agentFilter}
          onChange={(v) => setFilter('agent', v)}
          options={agentOptions}
          placeholder="All testers"
          ariaLabel="Filter by tester"
          className="w-40 md:w-48"
          compact
        />

        {/* Created By combobox — operator+ only */}
        {isOperator && memberOptions.length > 0 && (
          <Combobox
            value={createdByFilter}
            onChange={(v) => setFilter('created_by', v)}
            options={memberOptions}
            placeholder="All users"
            ariaLabel="Filter by user"
            className="w-40 md:w-48"
            compact
          />
        )}
      </FilterBar>

      {showCreate && (
        <CreateJobDialog projectId={projectId} onClose={() => setShowCreate(false)} onCreated={loadJobs} />
      )}

      {/* Testers Panel — flat list, no card wrapper */}
      {showTesters && (
        <div className="border-b border-gray-800/50 pb-5 mb-6 mt-4">
          <div className="flex items-center justify-between mb-3">
            <p className="text-xs text-gray-500 tracking-wider font-medium">testers</p>
            {isOperator && (
              <button
                onClick={() => setShowAddTester(!showAddTester)}
                className="text-xs text-cyan-400 hover:text-cyan-300"
              >
                {showAddTester ? 'Cancel' : '+ Add Tester'}
              </button>
            )}
          </div>

          {/* Add Tester Form — inline with left accent */}
          {showAddTester && isOperator && (
            <div className="border-l-2 border-cyan-500/30 pl-3 mb-4">
              <div className="flex gap-2 mb-3">
                {([
                  { id: 'cloud', label: 'New Cloud VM' },
                  { id: 'endpoint', label: 'Add to existing endpoint' },
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

              {addTesterMode === 'cloud' && (
                <div className="mb-2">
                  <p className="text-xs text-gray-500 mb-2">
                    Create a new Azure VM with the tester agent pre-installed. Takes ~3 minutes.
                  </p>
                  <div className="grid grid-cols-2 gap-2">
                    <div>
                      <label className="text-xs text-gray-500 mb-1 block">Region</label>
                      <select
                        value={cloudRegion}
                        onChange={e => setCloudRegion(e.target.value)}
                        className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        <option value="eastus">East US</option>
                        <option value="eastus2">East US 2</option>
                        <option value="westus2">West US 2</option>
                        <option value="westus3">West US 3</option>
                        <option value="northeurope">North Europe</option>
                        <option value="westeurope">West Europe</option>
                        <option value="southeastasia">Southeast Asia</option>
                        <option value="australiaeast">Australia East</option>
                        <option value="uksouth">UK South</option>
                        <option value="centralus">Central US</option>
                      </select>
                    </div>
                    <div>
                      <label className="text-xs text-gray-500 mb-1 block">VM Size</label>
                      <select
                        value={cloudVmSize}
                        onChange={e => setCloudVmSize(e.target.value)}
                        className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                      >
                        <option value="Standard_B1s">B1s (1 vCPU, 1 GB) — $0.01/hr</option>
                        <option value="Standard_B2s">B2s (2 vCPU, 4 GB) — $0.04/hr</option>
                        <option value="Standard_D2s_v3">D2s v3 (2 vCPU, 8 GB) — $0.10/hr</option>
                        <option value="Standard_D2s_v5">D2s v5 (2 vCPU, 8 GB) — $0.10/hr</option>
                      </select>
                    </div>
                  </div>
                </div>
              )}

              {addTesterMode === 'endpoint' && (
                <div className="mb-2">
                  <p className="text-xs text-gray-500 mb-2">
                    Add the tester agent to a VM you already deployed. No new VM created — just installs the tester software.
                  </p>
                  <select
                    value={selectedEndpoint}
                    onChange={e => setSelectedEndpoint(e.target.value)}
                    className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
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
                    className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                  <input
                    value={sshUser}
                    onChange={e => setSshUser(e.target.value)}
                    placeholder="SSH user"
                    className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                  <input
                    type="number"
                    value={sshPort}
                    onChange={e => setSshPort(Number(e.target.value))}
                    placeholder="Port"
                    className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </div>
              )}

              <div className="flex gap-2 items-center">
                <input
                  value={testerName}
                  onChange={e => setTesterName(e.target.value)}
                  placeholder="Tester name (optional)"
                  className="flex-1 bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                />
                <button
                  onClick={handleAddTester}
                  disabled={addingTester || (addTesterMode === 'ssh' && !sshHost.trim()) || (addTesterMode === 'endpoint' && !selectedEndpoint) || (addTesterMode === 'cloud' && !cloudRegion)}
                  className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
                >
                  {addingTester ? 'Adding...' : 'Deploy'}
                </button>
              </div>
            </div>
          )}

          {/* Tester List — flat rows with dividers */}
          {testers.length === 0 ? (
            <p className="text-gray-600 text-sm">No testers registered. Click "+ Add Tester" to deploy one on an endpoint or via SSH.</p>
          ) : (
            <div>
              {testers.map((t, i) => (
                <div
                  key={t.agent_id}
                  className={`flex items-center justify-between py-2.5 ${i > 0 ? 'border-t border-gray-800/30' : ''}`}
                >
                  <div className="flex items-center gap-3">
                    <StatusBadge status={t.status} />
                    <span className="text-sm text-gray-200">{t.name}</span>
                    <span className="text-xs text-gray-600">
                      {t.provider && <>{t.provider} </>}
                      {t.region && <>{t.region} </>}
                      {t.version && <>v{t.version} </>}
                      {t.last_heartbeat && (
                        <>seen {new Date(t.last_heartbeat).toLocaleTimeString()}</>
                      )}
                    </span>
                  </div>
                  {isOperator && (
                    <button
                      onClick={() => handleDeleteTester(t.agent_id, t.name)}
                      className="text-xs text-gray-600 hover:text-red-400 transition-colors"
                      title="Remove tester"
                      aria-label={`Remove tester ${t.name}`}
                    >
                      &#x2715;
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh test list. Retrying automatically.
        </div>
      )}

      {/* ── Mobile card layout (< md) ── */}
      <div className="md:hidden space-y-2 mt-4">
        {filteredJobs.length === 0 ? (
          testers.length === 0 ? (
            <div className="border border-gray-800 rounded p-8 text-center">
              <p className="text-gray-500 text-sm">No testers connected</p>
              <p className="text-gray-700 text-xs mt-1">Deploy a tester on one of your endpoints to start running diagnostics.</p>
              {isOperator && (
                <button
                  onClick={() => { setShowTesters(true); setShowAddTester(true); }}
                  className="text-xs text-cyan-400 mt-2"
                >
                  Add Tester
                </button>
              )}
            </div>
          ) : (
            <div className="border border-gray-800 rounded p-8 text-center">
              <p className="text-gray-500 text-sm">{activeFilterCount > 0 ? 'No tests match filters' : 'No tests yet'}</p>
              {isOperator && activeFilterCount === 0 && (
                <button onClick={() => setShowCreate(true)} className="text-xs text-cyan-400 mt-2">Run your first test</button>
              )}
            </div>
          )
        ) : filteredJobs.map((job) => {
          const isActive = job.status === 'running' || job.status === 'assigned';
          const duration = job.started_at
            ? formatDuration(new Date(job.started_at), job.finished_at ? new Date(job.finished_at) : new Date())
            : null;
          return (
            <Link
              key={job.job_id}
              to={jobResultLink(projectId, job) ?? `/projects/${projectId}/tests/${job.job_id}`}
              className={`block border border-gray-800 rounded p-3 ${isActive ? 'border-l-2 border-l-blue-500/50' : ''}`}
            >
              <div className="flex items-center justify-between mb-1">
                <div className="flex items-center gap-2 min-w-0">
                  <span className="text-cyan-400 font-mono text-xs">{job.job_id.slice(0, 8)}</span>
                  <span className={`text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded border ${isTlsProfileJob(job) ? 'border-cyan-500/30 text-cyan-300 bg-cyan-500/10' : 'border-gray-700 text-gray-500 bg-gray-800/30'}`}>
                    {jobLabel(job)}
                  </span>
                </div>
                <StatusBadge status={job.status} />
              </div>
              <p className="text-gray-300 text-xs truncate mb-1">{jobTarget(job)}</p>
              <div className="flex items-center gap-3 text-xs text-gray-500 flex-wrap">
                <span>{jobSummary(job)}</span>
                {job.tls_profile_run_id && <span className="text-cyan-400">View TLS result</span>}
                {!job.tls_profile_run_id && job.run_id && <span className="text-cyan-400">View run</span>}
                {duration && <span className="font-mono">{duration}</span>}
                <span>{new Date(job.created_at).toLocaleTimeString()}</span>
              </div>
            </Link>
          );
        })}
      </div>

      {/* ── Desktop/iPad table (≥ md) ── */}
      <div className="hidden md:block table-container mt-4">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Job</th>
              <th className="px-4 py-2.5 text-left font-medium">Target</th>
              <th className="px-4 py-2.5 text-left font-medium">Type</th>
              <th className="px-4 py-2.5 text-left font-medium">Summary</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Runs</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Tester</th>
              <th className="px-4 py-2.5 text-left font-medium">Status</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Duration</th>
              <th className="px-4 py-2.5 text-left font-medium">Created</th>
            </tr>
          </thead>
          <tbody>
            {filteredJobs.map((job) => {
              const isActive = job.status === 'running' || job.status === 'assigned';
              const duration = job.started_at
                ? formatDuration(
                    new Date(job.started_at),
                    job.finished_at ? new Date(job.finished_at) : new Date()
                  )
                : '-';
              const tName = job.agent_id
                ? testerMap.get(job.agent_id) || job.agent_id.slice(0, 8)
                : '-';
              return (
                <tr
                  key={job.job_id}
                  className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${isActive ? 'bg-blue-500/5' : ''}`}
                >
                  <td className="px-4 py-3">
                    <Link to={jobResultLink(projectId, job) ?? `/projects/${projectId}/tests/${job.job_id}`} className="text-cyan-400 hover:underline font-mono text-xs">
                      {job.job_id.slice(0, 8)}
                    </Link>
                  </td>
                  <td className="px-4 py-3 text-gray-300 text-xs max-w-48 truncate" title={jobTarget(job)}>
                    {jobTarget(job)}
                  </td>
                  <td className="px-4 py-3 text-xs">
                    <span className={`text-[10px] uppercase tracking-wider px-1.5 py-0.5 rounded border ${isTlsProfileJob(job) ? 'border-cyan-500/30 text-cyan-300 bg-cyan-500/10' : 'border-gray-700 text-gray-500 bg-gray-800/30'}`}>
                      {jobLabel(job)}
                    </span>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs max-w-40 truncate" title={jobSummary(job)}>
                    <div>{jobSummary(job)}</div>
                    {job.tls_profile_run_id && <div className="text-cyan-400 mt-1">Linked TLS result</div>}
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">{job.config?.runs ?? '-'}</td>
                  <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">{tName}</td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <StatusBadge status={job.status} />
                      {isActive && <span className="w-1.5 h-1.5 rounded-full bg-blue-400 motion-safe:animate-pulse" />}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs font-mono hidden lg:table-cell">{duration}</td>
                  <td className="px-4 py-3 text-gray-500 text-xs">{new Date(job.created_at).toLocaleString()}</td>
                </tr>
              );
            })}
          </tbody>
        </table>

        {filteredJobs.length === 0 && (
          testers.length === 0 ? (
            <div className="border border-gray-800 rounded p-8 text-center my-6">
              <p className="text-gray-500 text-sm">No testers connected</p>
              <p className="text-gray-700 text-xs mt-1">Deploy a tester on one of your endpoints to start running diagnostics.</p>
              {isOperator && (
                <button
                  onClick={() => { setShowTesters(true); setShowAddTester(true); }}
                  className="text-xs text-cyan-400 mt-2"
                >
                  Add Tester
                </button>
              )}
            </div>
          ) : (
            <div className="py-10 text-center">
              <p className="text-gray-500 text-sm">
                {activeFilterCount > 0 ? 'No tests match the current filters' : 'No tests yet — click New Test to run your first diagnostic'}
              </p>
            </div>
          )
        )}
      </div>
    </div>
  );
}
