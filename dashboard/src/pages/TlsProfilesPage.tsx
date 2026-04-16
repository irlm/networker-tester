import { useCallback, useState } from 'react';
import { Link } from 'react-router-dom';
import { api, type TlsProfileSummary } from '../api/client';
import { CreateTlsProfileDialog } from '../components/CreateTlsProfileDialog';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

export function TlsProfilesPage() {
  const { projectId } = useProject();
  const [profiles, setProfiles] = useState<TlsProfileSummary[]>([]);
  const [showCreate, setShowCreate] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [hostSearch, setHostSearch] = useState('');
  const [kindFilter, setKindFilter] = useState<'all' | 'managed-endpoint' | 'external-url' | 'external-host'>('all');
  const [statusFilter, setStatusFilter] = useState<'all' | 'pass' | 'warn' | 'fail'>('all');

  usePageTitle('TLS Profiles');

  const loadProfiles = useCallback(() => {
    if (!projectId) return;
    api
      .getTlsProfiles(projectId, { limit: 100 })
      .then((data) => {
        setProfiles(data);
        setError(null);
        setLoading(false);
      })
      .catch((e) => {
        setError(String(e));
        setLoading(false);
      });
  }, [projectId]);

  usePolling(loadProfiles, 15000);

  const filtered = profiles.filter((p) => {
    const matchesHost = hostSearch.trim()
      ? `${p.host}:${p.port}`.toLowerCase().includes(hostSearch.trim().toLowerCase())
      : true;
    const matchesKind = kindFilter === 'all' ? true : p.target_kind === kindFilter;
    const normalizedStatus = p.summary_status.toLowerCase();
    const matchesStatus = statusFilter === 'all'
      ? true
      : statusFilter === 'pass'
        ? normalizedStatus.includes('pass') || normalizedStatus.includes('ok') || normalizedStatus.includes('good')
        : statusFilter === 'warn'
          ? normalizedStatus.includes('warn') || normalizedStatus.includes('partial')
          : normalizedStatus.includes('fail') || normalizedStatus.includes('error');
    return matchesHost && matchesKind && matchesStatus;
  });

  if (loading && profiles.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">TLS Profiles</h2>
        <div className="text-gray-500 motion-safe:animate-pulse">Loading TLS profile history...</div>
      </div>
    );
  }

  if (error && profiles.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-6">TLS Profiles</h2>
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load TLS profiles</h3>
          <p className="text-red-300 text-sm">Could not fetch TLS profile history. Check your connection and try refreshing.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6 gap-2">
        <div>
          <h2 className="text-lg md:text-xl font-bold text-gray-100">TLS Profiles</h2>
          <p className="text-xs text-gray-500 mt-1">Persisted target TLS observations and history.</p>
        </div>
        <div className="flex items-center gap-2 flex-wrap justify-end">
          <input
            type="search"
            value={hostSearch}
            onChange={(e) => setHostSearch(e.target.value)}
            placeholder="Filter by host..."
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-40 md:w-64 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
          />
          <select
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value as 'all' | 'managed-endpoint' | 'external-url' | 'external-host')}
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
          >
            <option value="all">All kinds</option>
            <option value="managed-endpoint">Managed target</option>
            <option value="external-url">External URL</option>
            <option value="external-host">External host</option>
          </select>
          <select
            value={statusFilter}
            onChange={(e) => setStatusFilter(e.target.value as 'all' | 'pass' | 'warn' | 'fail')}
            className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
          >
            <option value="all">All statuses</option>
            <option value="pass">Pass</option>
            <option value="warn">Warn</option>
            <option value="fail">Fail</option>
          </select>
          <button
            onClick={() => setShowCreate(true)}
            className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-1.5 rounded text-sm transition-colors"
          >
            Run TLS Profile
          </button>
        </div>
      </div>

      {showCreate && projectId && (
        <CreateTlsProfileDialog projectId={projectId} onClose={() => setShowCreate(false)} onCreated={loadProfiles} />
      )}

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh TLS profiles. Retrying automatically.
        </div>
      )}

      <div className="text-xs text-gray-500 mb-3">
        Showing {filtered.length} of {profiles.length} TLS profile runs
      </div>

      <div className="md:hidden space-y-2">
        {filtered.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">No TLS profile runs yet</p>
            <p className="text-gray-700 text-xs mt-1">Run a TLS endpoint profile with DB saving enabled and it will show up here.</p>
          </div>
        ) : filtered.map((profile) => (
          <Link
            key={profile.id}
            to={`/projects/${projectId}/tls-profiles/${profile.id}`}
            className="block border border-gray-800 rounded p-3"
          >
            <div className="flex items-center justify-between mb-1 gap-2">
              <span className="text-cyan-400 font-medium text-xs truncate">{profile.host}:{profile.port}</span>
              <span className="text-xs text-gray-500">{new Date(profile.started_at).toLocaleTimeString()}</span>
            </div>
            <div className="flex flex-wrap gap-2 text-xs">
              <span className="text-gray-400">{profile.target_kind}</span>
              <span className="text-gray-500">{profile.coverage_level}</span>
              <span className={`font-medium ${statusClass(profile.summary_status)}`}>{profile.summary_status}</span>
              {profile.summary_score != null && <span className="text-amber-400">score {profile.summary_score}</span>}
            </div>
          </Link>
        ))}
      </div>

      <div className="hidden md:block table-container">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Target</th>
              <th className="px-4 py-2.5 text-left font-medium">Kind</th>
              <th className="px-4 py-2.5 text-left font-medium">Coverage</th>
              <th className="px-4 py-2.5 text-left font-medium">Status</th>
              <th className="px-4 py-2.5 text-left font-medium">Score</th>
              <th className="px-4 py-2.5 text-left font-medium">Started</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((profile) => (
              <tr key={profile.id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                <td className="px-4 py-3">
                  <Link to={`/projects/${projectId}/tls-profiles/${profile.id}`} className="text-cyan-400 hover:underline text-xs font-medium">
                    {profile.host}:{profile.port}
                  </Link>
                </td>
                <td className="px-4 py-3 text-gray-300 text-xs">{profile.target_kind}</td>
                <td className="px-4 py-3 text-gray-500 text-xs">{profile.coverage_level}</td>
                <td className={`px-4 py-3 text-xs font-medium ${statusClass(profile.summary_status)}`}>{profile.summary_status}</td>
                <td className="px-4 py-3 text-amber-400 text-xs">{profile.summary_score ?? '-'}</td>
                <td className="px-4 py-3 text-gray-500 text-xs">{new Date(profile.started_at).toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>

        {filtered.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">No TLS profile runs yet</p>
            <p className="text-gray-700 text-xs mt-1">Run a TLS endpoint profile with DB saving enabled and it will show up here.</p>
          </div>
        )}
      </div>
    </div>
  );
}

function statusClass(status: string) {
  const normalized = status.toLowerCase();
  if (normalized.includes('pass') || normalized.includes('ok') || normalized.includes('good')) return 'text-green-400';
  if (normalized.includes('warn') || normalized.includes('partial')) return 'text-yellow-400';
  if (normalized.includes('fail') || normalized.includes('error')) return 'text-red-400';
  return 'text-gray-300';
}
