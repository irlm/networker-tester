import { useState, useCallback, useRef, useEffect } from 'react';
import { api, type SystemMetrics, type DbMetrics, type WorkspaceUsage, type LogEntry } from '../api/client';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';

type Tab = 'overview' | 'usage' | 'logs';

// ── Helpers ────────────────────────────────────────────────────────────

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatUptime(s: number): string {
  const days = Math.floor(s / 86400);
  const hours = Math.floor((s % 86400) / 3600);
  const mins = Math.floor((s % 3600) / 60);
  if (days > 0) return `${days}d ${hours}h ${mins}m`;
  if (hours > 0) return `${hours}h ${mins}m`;
  return `${mins}m`;
}

function barColor(pct: number): string {
  if (pct >= 90) return 'bg-red-500';
  if (pct >= 70) return 'bg-yellow-500';
  return 'bg-green-500';
}

function timeAgo(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function daysSince(dateStr: string): number {
  return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
}

// ── Progress bar ───────────────────────────────────────────────────────

function ProgressBar({ label, value, max, unit }: { label: string; value: number; max: number; unit?: string }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  return (
    <div className="mb-3">
      <div className="flex items-center justify-between mb-1">
        <span className="text-xs text-gray-400">{label}</span>
        <span className="text-xs text-gray-500 font-mono">
          {unit ? `${formatBytes(value)} / ${formatBytes(max)}` : `${pct.toFixed(1)}%`}
        </span>
      </div>
      <div className="h-2 bg-gray-800 rounded-full overflow-hidden">
        <div className={`h-full rounded-full transition-all duration-500 ${barColor(pct)}`} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

// ── Status badge ───────────────────────────────────────────────────────

function workspaceStatus(ws: WorkspaceUsage): { label: string; cls: string } {
  if (ws.deleted_at) return { label: 'suspended', cls: 'bg-gray-500/20 text-gray-400' };
  if (ws.last_activity) {
    const days = daysSince(ws.last_activity);
    if (days > 90) return { label: 'warning', cls: 'bg-red-500/20 text-red-400' };
    if (days > 60) return { label: 'at-risk', cls: 'bg-yellow-500/20 text-yellow-400' };
  }
  return { label: 'active', cls: 'bg-green-500/20 text-green-400' };
}

// ── Overview Tab ───────────────────────────────────────────────────────

function OverviewTab({ system, db, version, userCount, workspaceCount }: {
  system: SystemMetrics | null;
  db: DbMetrics | null;
  version: string | null;
  userCount: number;
  workspaceCount: number;
}) {
  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
      {/* Server Metrics */}
      <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
        <div className="text-xs text-gray-500 tracking-wider font-medium mb-3">SERVER</div>
        {system ? (
          <>
            <ProgressBar label="CPU" value={system.cpu_usage_percent} max={100} />
            <ProgressBar label="Memory" value={system.memory_used_bytes} max={system.memory_total_bytes} unit="bytes" />
            <ProgressBar label="Disk" value={system.disk_used_bytes} max={system.disk_total_bytes} unit="bytes" />
            <div className="flex items-center justify-between mt-2">
              <span className="text-xs text-gray-400">Uptime</span>
              <span className="text-xs text-gray-300 font-mono">{formatUptime(system.uptime_seconds)}</span>
            </div>
          </>
        ) : (
          <p className="text-sm text-gray-600">Loading...</p>
        )}
      </div>

      {/* Database Metrics */}
      <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
        <div className="text-xs text-gray-500 tracking-wider font-medium mb-3">DATABASE</div>
        {db ? (
          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-400">Connections</span>
              <span className="text-xs text-gray-300 font-mono">{db.active_connections} / {db.max_connections}</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-400">Database size</span>
              <span className="text-xs text-gray-300 font-mono">{formatBytes(db.database_size_bytes)}</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-400">Cache hit ratio</span>
              <span className="text-xs text-gray-300 font-mono">{(db.cache_hit_ratio * 100).toFixed(2)}%</span>
            </div>
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-400">Oldest transaction</span>
              <span className="text-xs text-gray-300 font-mono">
                {db.oldest_transaction_age_seconds !== null ? `${db.oldest_transaction_age_seconds}s` : 'none'}
              </span>
            </div>
          </div>
        ) : (
          <p className="text-sm text-gray-600">Loading...</p>
        )}
      </div>

      {/* Application */}
      <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4 lg:col-span-2">
        <div className="text-xs text-gray-500 tracking-wider font-medium mb-3">APPLICATION</div>
        <div className="grid grid-cols-3 gap-4">
          <div>
            <span className="text-xs text-gray-400 block">Version</span>
            <span className="text-sm text-gray-200 font-mono">{version ?? '...'}</span>
          </div>
          <div>
            <span className="text-xs text-gray-400 block">Total users</span>
            <span className="text-sm text-gray-200 font-mono">{userCount}</span>
          </div>
          <div>
            <span className="text-xs text-gray-400 block">Total workspaces</span>
            <span className="text-sm text-gray-200 font-mono">{workspaceCount}</span>
          </div>
        </div>
      </div>
    </div>
  );
}

// ── Usage Tab ──────────────────────────────────────────────────────────

function UsageTab({ workspaces, onRefresh }: { workspaces: WorkspaceUsage[]; onRefresh: () => void }) {
  const toast = useToast();

  const handleSuspend = async (projectId: string, name: string) => {
    try {
      await api.suspendWorkspace(projectId);
      toast('success', `Suspended ${name}`);
      onRefresh();
    } catch { toast('error', 'Failed to suspend workspace'); }
  };

  const handleRestore = async (projectId: string, name: string) => {
    try {
      await api.restoreWorkspace(projectId);
      toast('success', `Restored ${name}`);
      onRefresh();
    } catch { toast('error', 'Failed to restore workspace'); }
  };

  const handleProtect = async (projectId: string) => {
    try {
      const res = await api.protectWorkspace(projectId);
      toast('success', res.delete_protection ? 'Protection enabled' : 'Protection disabled');
      onRefresh();
    } catch { toast('error', 'Failed to toggle protection'); }
  };

  const handleDelete = async (projectId: string, name: string) => {
    if (!window.confirm(`Permanently delete workspace "${name}"? This cannot be undone.`)) return;
    try {
      await api.hardDeleteWorkspace(projectId);
      toast('success', `Deleted ${name}`);
      onRefresh();
    } catch { toast('error', 'Failed to delete workspace'); }
  };

  return (
    <div className="table-container">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-left text-xs text-gray-500 border-b border-gray-800">
            <th className="pb-2 pr-3 font-medium">Name</th>
            <th className="pb-2 pr-3 font-medium">Members</th>
            <th className="pb-2 pr-3 font-medium">Testers</th>
            <th className="pb-2 pr-3 font-medium">Jobs (30d)</th>
            <th className="pb-2 pr-3 font-medium">Runs (30d)</th>
            <th className="pb-2 pr-3 font-medium">Last Activity</th>
            <th className="pb-2 pr-3 font-medium">Status</th>
            <th className="pb-2 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          {workspaces.map((ws) => {
            const st = workspaceStatus(ws);
            const isSuspended = ws.deleted_at !== null;
            return (
              <tr key={ws.project_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                <td className="py-2 pr-3 text-gray-200 font-mono">
                  {ws.name}
                  {ws.delete_protection && (
                    <span className="ml-1.5 text-yellow-500 text-xs" title="Delete protection enabled">&#9737;</span>
                  )}
                </td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{ws.member_count}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{ws.tester_count}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{ws.jobs_30d}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{ws.runs_30d}</td>
                <td className="py-2 pr-3 text-gray-500 text-xs">
                  {ws.last_activity ? timeAgo(ws.last_activity) : 'never'}
                </td>
                <td className="py-2 pr-3">
                  <span className={`text-[10px] px-1.5 py-0.5 rounded ${st.cls}`}>{st.label}</span>
                </td>
                <td className="py-2">
                  <div className="flex items-center gap-1.5">
                    {!isSuspended ? (
                      <button
                        onClick={() => handleSuspend(ws.project_id, ws.name)}
                        className="px-2 py-1 text-xs rounded text-yellow-400 hover:bg-yellow-500/20 transition-colors"
                        title="Suspend"
                      >
                        Suspend
                      </button>
                    ) : (
                      <button
                        onClick={() => handleRestore(ws.project_id, ws.name)}
                        className="px-2 py-1 text-xs rounded text-green-400 hover:bg-green-500/20 transition-colors"
                        title="Restore"
                      >
                        Restore
                      </button>
                    )}
                    <button
                      onClick={() => handleProtect(ws.project_id)}
                      className={`px-1.5 py-1 text-xs rounded transition-colors ${
                        ws.delete_protection
                          ? 'text-yellow-500 hover:bg-yellow-500/20'
                          : 'text-gray-500 hover:bg-gray-700/50 hover:text-gray-300'
                      }`}
                      title={ws.delete_protection ? 'Remove delete protection' : 'Enable delete protection'}
                    >
                      &#9737;
                    </button>
                    {isSuspended && (
                      <button
                        onClick={() => handleDelete(ws.project_id, ws.name)}
                        className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors"
                        title="Permanently delete"
                      >
                        Delete
                      </button>
                    )}
                  </div>
                </td>
              </tr>
            );
          })}
          {workspaces.length === 0 && (
            <tr>
              <td colSpan={8} className="py-8 text-center text-gray-500 text-sm">No workspaces</td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

// ── Logs Tab ───────────────────────────────────────────────────────────

function LogsTab() {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [level, setLevel] = useState('');
  const [search, setSearch] = useState('');
  const [paused, setPaused] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const fetchLogs = useCallback(async () => {
    try {
      const data = await api.getSystemLogs({
        level: level || undefined,
        search: search || undefined,
        limit: 500,
      });
      setLogs(data);
    } catch {
      // retry on next poll
    }
  }, [level, search]);

  usePolling(fetchLogs, 5000);

  // Auto-scroll to bottom when logs update (unless paused)
  useEffect(() => {
    if (!paused && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [logs, paused]);

  const levelColor = (l: string) => {
    switch (l.toUpperCase()) {
      case 'ERROR': return 'text-red-400';
      case 'WARN': return 'text-yellow-400';
      default: return 'text-gray-400';
    }
  };

  const formatTime = (ts: string) => {
    try {
      const d = new Date(ts);
      return d.toTimeString().slice(0, 8);
    } catch {
      return ts;
    }
  };

  return (
    <div>
      <div className="flex items-center gap-3 mb-3 flex-wrap">
        <select
          value={level}
          onChange={(e) => setLevel(e.target.value)}
          className="text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-gray-300"
        >
          <option value="">All</option>
          <option value="INFO">INFO</option>
          <option value="WARN">WARN</option>
          <option value="ERROR">ERROR</option>
        </select>
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search logs..."
          className="flex-1 min-w-[200px] bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
        />
        <button
          onClick={() => setPaused(!paused)}
          className={`px-3 py-1.5 text-xs rounded border transition-colors ${
            paused
              ? 'border-yellow-600 text-yellow-400 bg-yellow-500/10'
              : 'border-gray-700 text-gray-400 hover:text-gray-200'
          }`}
        >
          {paused ? 'Paused' : 'Pause'}
        </button>
      </div>
      <div
        ref={containerRef}
        className="max-h-[600px] overflow-y-auto border border-gray-800 rounded bg-[var(--bg-card)] p-3 font-mono text-xs leading-relaxed"
      >
        {logs.length === 0 && (
          <p className="text-gray-600 text-center py-4">No log entries</p>
        )}
        {logs.map((entry, i) => (
          <div key={i} className={levelColor(entry.level)}>
            <span className="text-gray-600">[{formatTime(entry.timestamp)}]</span>{' '}
            <span className="font-bold">{entry.level.padEnd(5)}</span>{' '}
            <span className="text-gray-500">{entry.target}</span>{' '}
            &mdash; {entry.message}
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Main Page ──────────────────────────────────────────────────────────

export function SystemDashboardPage() {
  usePageTitle('System');
  const [tab, setTab] = useState<Tab>('overview');

  // Overview state
  const [system, setSystem] = useState<SystemMetrics | null>(null);
  const [db, setDb] = useState<DbMetrics | null>(null);
  const [version, setVersion] = useState<string | null>(null);
  const [userCount, setUserCount] = useState(0);
  const [workspaces, setWorkspaces] = useState<WorkspaceUsage[]>([]);

  const refreshMetrics = useCallback(async () => {
    try {
      const [metrics, ver, users, ws] = await Promise.all([
        api.getSystemMetrics(),
        api.getVersionInfo(),
        api.getUsers(),
        api.getWorkspaceUsage(),
      ]);
      setSystem(metrics.system);
      setDb(metrics.db);
      setVersion(ver.dashboard_version);
      setUserCount(users.length);
      setWorkspaces(ws);
    } catch {
      // retry on next poll
    }
  }, []);

  usePolling(refreshMetrics, 30000, tab === 'overview' || tab === 'usage');

  const refreshWorkspaces = useCallback(async () => {
    try {
      setWorkspaces(await api.getWorkspaceUsage());
    } catch { /* retry */ }
  }, []);

  const tabCls = (t: Tab) =>
    `px-4 py-2 text-sm rounded-t transition-colors ${
      tab === t
        ? 'bg-gray-800/40 text-gray-100'
        : 'text-gray-400 hover:text-gray-200'
    }`;

  return (
    <div className="p-4 md:p-6 max-w-6xl">
      <h1 className="text-lg md:text-xl font-bold text-gray-100 mb-4">System</h1>

      {/* Tab selector */}
      <div className="flex gap-1 mb-4">
        <button onClick={() => setTab('overview')} className={tabCls('overview')}>Overview</button>
        <button onClick={() => setTab('usage')} className={tabCls('usage')}>Usage</button>
        <button onClick={() => setTab('logs')} className={tabCls('logs')}>Logs</button>
      </div>

      {tab === 'overview' && (
        <OverviewTab
          system={system}
          db={db}
          version={version}
          userCount={userCount}
          workspaceCount={workspaces.length}
        />
      )}

      {tab === 'usage' && (
        <UsageTab workspaces={workspaces} onRefresh={refreshWorkspaces} />
      )}

      {tab === 'logs' && <LogsTab />}
    </div>
  );
}
