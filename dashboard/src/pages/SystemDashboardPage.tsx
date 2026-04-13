import { useState, useCallback, useRef, useEffect } from 'react';
import { api, type SystemMetrics, type DbMetrics, type WorkspaceUsage, type LogEntry, type SsoProvider, type CreateSsoProvider } from '../api/client';

// ── Log helpers ─────────────────────────────────────────────────────────

function levelToString(level: number): string {
  switch (level) {
    case 1: return 'ERROR';
    case 2: return 'WARN';
    case 3: return 'INFO';
    case 4: return 'DEBUG';
    case 5: return 'TRACE';
    default: return `L${level}`;
  }
}
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';

type Tab = 'overview' | 'usage' | 'logs' | 'auth';

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

const SERVICES = ['dashboard', 'orchestrator', 'agent', 'tester', 'endpoint'] as const;

function LogsTab() {
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [level, setLevel] = useState('');
  const [service, setService] = useState('');
  const [search, setSearch] = useState('');
  const [paused, setPaused] = useState(false);
  const [truncated, setTruncated] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const fetchLogs = useCallback(async () => {
    try {
      const data = await api.getSystemLogs({
        level: level || undefined,
        service: service || undefined,
        search: search || undefined,
        limit: 500,
      });
      setLogs(data.entries);
      setTruncated(data.truncated);
    } catch {
      // retry on next poll
    }
  }, [level, service, search]);

  usePolling(fetchLogs, 5000);

  // Auto-scroll to bottom when logs update (unless paused)
  useEffect(() => {
    if (!paused && containerRef.current) {
      containerRef.current.scrollTop = containerRef.current.scrollHeight;
    }
  }, [logs, paused]);

  const levelColor = (l: number) => {
    switch (l) {
      case 1: return 'text-red-400';
      case 2: return 'text-yellow-400';
      case 4: return 'text-gray-500';
      case 5: return 'text-gray-600';
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
          <option value="">All levels</option>
          <option value="error">ERROR</option>
          <option value="warn">WARN</option>
          <option value="info">INFO</option>
          <option value="debug">DEBUG</option>
          <option value="trace">TRACE</option>
        </select>
        <select
          value={service}
          onChange={(e) => setService(e.target.value)}
          className="text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-gray-300"
        >
          <option value="">All services</option>
          {SERVICES.map(s => (
            <option key={s} value={s}>{s}</option>
          ))}
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
      {truncated && (
        <p className="text-xs text-yellow-500/70 mb-2">Showing latest 500 entries — results truncated</p>
      )}
      <div
        ref={containerRef}
        className="max-h-[600px] overflow-y-auto border border-gray-800 rounded bg-[var(--bg-card)] p-3 font-mono text-xs leading-relaxed"
      >
        {logs.length === 0 && (
          <p className="text-gray-600 text-center py-4">No log entries</p>
        )}
        {logs.map((entry, i) => {
          const levelStr = levelToString(entry.level);
          return (
            <div key={i} className={levelColor(entry.level)}>
              <span className="text-gray-600">[{formatTime(entry.ts)}]</span>{' '}
              <span className="font-bold">{levelStr.padEnd(5)}</span>{' '}
              <span className="text-gray-500">{entry.service}</span>{' '}
              &mdash; {entry.message}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── Auth Tab ──────────────────────────────────────────────────────────

interface FieldDef {
  key: string;
  label: string;
  required?: boolean;
  secret?: boolean;
  help?: string;
}

const PROVIDER_TYPES: { value: string; label: string }[] = [
  { value: 'microsoft', label: 'Microsoft Entra ID' },
  { value: 'google', label: 'Google' },
  { value: 'oidc_generic', label: 'Generic OIDC' },
];

const PROVIDER_FIELDS: Record<string, FieldDef[]> = {
  microsoft: [
    { key: 'client_id', label: 'Application (client) ID', required: true,
      help: 'Entra admin center \u2192 App registrations \u2192 your app \u2192 Overview \u2192 Application (client) ID' },
    { key: 'client_secret', label: 'Client Secret', required: true, secret: true,
      help: 'Same app \u2192 Certificates & secrets \u2192 New client secret. Copy the Value immediately \u2014 it won\'t be shown again.' },
    { key: 'tenant_id', label: 'Directory (tenant) ID', required: true,
      help: 'Same Overview page \u2192 Directory (tenant) ID. Use "common" to allow any Microsoft account.' },
  ],
  google: [
    { key: 'client_id', label: 'Client ID', required: true,
      help: 'Google Cloud Console \u2192 APIs & Services \u2192 Credentials \u2192 your OAuth 2.0 Client ID' },
    { key: 'client_secret', label: 'Client Secret', required: true, secret: true,
      help: 'Same credentials page \u2192 click the client \u2192 Client secret on the right.' },
  ],
  oidc_generic: [
    { key: 'client_id', label: 'Client ID', required: true,
      help: 'Found in your identity provider\'s app settings (e.g. Okta: Applications \u2192 your app \u2192 General \u2192 Client ID)' },
    { key: 'client_secret', label: 'Client Secret', required: true, secret: true,
      help: 'Same app settings page. For Okta: General tab \u2192 Client Credentials section.' },
    { key: 'issuer_url', label: 'Issuer URL', required: true,
      help: 'Base URL of your provider (e.g. https://your-domain.okta.com). Must serve /.well-known/openid-configuration' },
  ],
};

const PROVIDER_SETUP_GUIDES: Record<string, string[]> = {
  microsoft: [
    '1. Go to entra.microsoft.com \u2192 App registrations \u2192 New registration',
    '2. Name: "AletheDash SSO", choose your tenant scope',
    '3. Redirect URI (Web): {public_url}/api/auth/sso/callback',
    '4. Copy Application (client) ID and Directory (tenant) ID from Overview',
    '5. Certificates & secrets \u2192 New client secret \u2192 copy the Value immediately',
  ],
  google: [
    '1. Go to console.cloud.google.com \u2192 APIs & Services \u2192 Credentials',
    '2. Create Credentials \u2192 OAuth client ID \u2192 Web application',
    '3. Authorized redirect URI: {public_url}/api/auth/sso/callback',
    '4. Copy Client ID and Client secret shown after creation',
    '5. Enable the Google People API (APIs & Services \u2192 Library)',
  ],
  oidc_generic: [
    '1. In your identity provider (Okta, Auth0, Keycloak, etc.), create a new OIDC app',
    '2. Set redirect/callback URI: {public_url}/api/auth/sso/callback',
    '3. Copy the Client ID and Client Secret from app settings',
    '4. Find the Issuer URL (e.g. https://your-domain.okta.com)',
    '5. Verify: {issuer_url}/.well-known/openid-configuration returns JSON',
  ],
};

const DEFAULT_NAMES: Record<string, string> = {
  microsoft: 'Microsoft Entra ID',
  google: 'Google',
  oidc_generic: 'SSO',
};

function AuthTab() {
  const [providers, setProviders] = useState<SsoProvider[]>([]);
  const [loading, setLoading] = useState(true);
  const [publicUrl, setPublicUrl] = useState('');
  const [publicUrlSaving, setPublicUrlSaving] = useState(false);
  const [showForm, setShowForm] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [formType, setFormType] = useState('microsoft');
  const [formName, setFormName] = useState('');
  const [formFields, setFormFields] = useState<Record<string, string>>({});
  const [formEnabled, setFormEnabled] = useState(true);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const toast = useToast();

  const loadData = useCallback(async () => {
    try {
      const [provs, config] = await Promise.all([
        api.getSsoProviders(),
        api.getSystemConfig('public_url'),
      ]);
      setProviders(provs);
      if (config) setPublicUrl(config.value);
    } catch {
      toast('error', 'Failed to load auth settings');
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => { loadData(); }, [loadData]);

  const resetForm = () => {
    setShowForm(false);
    setEditingId(null);
    setFormType('microsoft');
    setFormName('');
    setFormFields({});
    setFormEnabled(true);
  };

  const openCreate = () => {
    resetForm();
    setFormName(DEFAULT_NAMES['microsoft']);
    setShowForm(true);
  };

  const openEdit = (p: SsoProvider) => {
    setEditingId(p.provider_id);
    setFormType(p.provider_type);
    setFormName(p.name);
    setFormEnabled(p.enabled);
    const fields: Record<string, string> = { client_id: p.client_id };
    if (p.tenant_id) fields.tenant_id = p.tenant_id;
    if (p.issuer_url) fields.issuer_url = p.issuer_url;
    setFormFields(fields);
    setShowForm(true);
  };

  const handleTypeChange = (type: string) => {
    setFormType(type);
    if (!editingId) {
      setFormName(DEFAULT_NAMES[type] || 'SSO');
      setFormFields({});
    }
  };

  const handleSave = async () => {
    const fields = PROVIDER_FIELDS[formType] || [];
    for (const f of fields) {
      if (f.required && !f.secret && !formFields[f.key]?.trim()) {
        toast('error', `${f.label} is required`);
        return;
      }
    }
    if (!editingId) {
      // Create: secret is required
      if (!formFields['client_secret']?.trim()) {
        toast('error', 'Client Secret is required');
        return;
      }
    }

    setSaving(true);
    try {
      const data: Partial<CreateSsoProvider> = {
        name: formName.trim(),
        provider_type: formType,
        client_id: formFields['client_id']?.trim(),
        enabled: formEnabled,
      };
      if (formFields['tenant_id']) data.tenant_id = formFields['tenant_id'].trim();
      if (formFields['issuer_url']) data.issuer_url = formFields['issuer_url'].trim();
      if (formFields['client_secret']?.trim()) data.client_secret = formFields['client_secret'].trim();

      if (editingId) {
        await api.updateSsoProvider(editingId, data);
        toast('success', 'Provider updated');
      } else {
        await api.createSsoProvider(data as CreateSsoProvider);
        toast('success', 'Provider created');
      }
      resetForm();
      await loadData();
    } catch {
      toast('error', 'Failed to save provider');
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await api.deleteSsoProvider(id);
      toast('success', 'Provider deleted');
      setConfirmDelete(null);
      await loadData();
    } catch {
      toast('error', 'Failed to delete provider');
    }
  };

  const handlePublicUrlSave = async () => {
    setPublicUrlSaving(true);
    try {
      await api.setSystemConfig('public_url', publicUrl.trim());
      toast('success', 'Public URL saved');
    } catch {
      toast('error', 'Failed to save public URL');
    } finally {
      setPublicUrlSaving(false);
    }
  };

  if (loading) return <p className="text-sm text-gray-600">Loading...</p>;

  return (
    <div className="space-y-6">
      {/* Public URL */}
      <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
        <div className="text-xs text-gray-500 tracking-wider font-medium mb-3">PUBLIC URL</div>
        <p className="text-xs text-gray-500 mb-2">
          Base URL for SSO redirect callbacks (e.g. https://dash.example.com).
        </p>
        <div className="flex items-center gap-2">
          <input
            type="url"
            value={publicUrl}
            onChange={e => setPublicUrl(e.target.value)}
            placeholder="https://dash.example.com"
            className="flex-1 bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
          />
          <button
            onClick={handlePublicUrlSave}
            disabled={publicUrlSaving}
            className="px-3 py-1.5 text-xs rounded border border-gray-700 text-gray-300 hover:text-gray-100 hover:border-gray-600 transition-colors disabled:opacity-50"
          >
            {publicUrlSaving ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>

      {/* Providers */}
      <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
        <div className="flex items-center justify-between mb-3">
          <div className="text-xs text-gray-500 tracking-wider font-medium">SSO PROVIDERS</div>
          <button
            onClick={openCreate}
            className="px-3 py-1.5 text-xs rounded border border-cyan-700 text-cyan-400 hover:bg-cyan-500/10 transition-colors"
          >
            + Add Provider
          </button>
        </div>

        {providers.length === 0 && !showForm && (
          <p className="text-sm text-gray-600 py-4 text-center">No SSO providers configured</p>
        )}

        {providers.length > 0 && (
          <table className="w-full text-sm mb-4">
            <thead>
              <tr className="text-left text-xs text-gray-500 border-b border-gray-800">
                <th className="pb-2 pr-3 font-medium">Name</th>
                <th className="pb-2 pr-3 font-medium">Type</th>
                <th className="pb-2 pr-3 font-medium">Client ID</th>
                <th className="pb-2 pr-3 font-medium">Status</th>
                <th className="pb-2 font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {providers.map(p => (
                <tr key={p.provider_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                  <td className="py-2 pr-3 text-gray-200 font-mono text-xs">{p.name}</td>
                  <td className="py-2 pr-3 text-gray-400 text-xs">{p.provider_type}</td>
                  <td className="py-2 pr-3 text-gray-500 font-mono text-xs truncate max-w-[200px]">{p.client_id}</td>
                  <td className="py-2 pr-3">
                    <span className={`text-[10px] px-1.5 py-0.5 rounded ${
                      p.enabled ? 'bg-green-500/20 text-green-400' : 'bg-gray-500/20 text-gray-400'
                    }`}>
                      {p.enabled ? 'enabled' : 'disabled'}
                    </span>
                  </td>
                  <td className="py-2">
                    <div className="flex items-center gap-1.5">
                      <button
                        onClick={() => openEdit(p)}
                        className="px-2 py-1 text-xs rounded text-cyan-400 hover:bg-cyan-500/20 transition-colors"
                      >
                        Edit
                      </button>
                      {confirmDelete === p.provider_id ? (
                        <>
                          <button
                            onClick={() => handleDelete(p.provider_id)}
                            className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors"
                          >
                            Confirm
                          </button>
                          <button
                            onClick={() => setConfirmDelete(null)}
                            className="px-2 py-1 text-xs rounded text-gray-500 hover:text-gray-300 transition-colors"
                          >
                            Cancel
                          </button>
                        </>
                      ) : (
                        <button
                          onClick={() => setConfirmDelete(p.provider_id)}
                          className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors"
                        >
                          Delete
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}

        {/* Create/Edit form */}
        {showForm && (
          <div className="border border-gray-700 rounded p-4 bg-[var(--bg-surface)]">
            <div className="text-xs text-gray-400 font-medium mb-3">
              {editingId ? 'Edit Provider' : 'New Provider'}
            </div>

            <div className="grid grid-cols-2 gap-4 mb-4">
              <div>
                <label className="block text-xs text-gray-500 mb-1">Provider Type</label>
                <select
                  value={formType}
                  onChange={e => handleTypeChange(e.target.value)}
                  disabled={!!editingId}
                  className="w-full text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-gray-300 disabled:opacity-50"
                >
                  {PROVIDER_TYPES.map(t => (
                    <option key={t.value} value={t.value}>{t.label}</option>
                  ))}
                </select>
              </div>
              <div>
                <label className="block text-xs text-gray-500 mb-1">Display Name</label>
                <input
                  value={formName}
                  onChange={e => setFormName(e.target.value)}
                  className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none font-mono"
                />
              </div>
            </div>

            {/* Setup guide */}
            {!editingId && PROVIDER_SETUP_GUIDES[formType] && (
              <div className="mb-4 bg-gray-900/40 border border-gray-800 rounded p-3">
                <div className="text-[10px] text-cyan-400/80 font-medium uppercase tracking-wider mb-1.5">Setup Guide</div>
                <ol className="text-[11px] text-gray-400 space-y-0.5 list-none pl-0">
                  {PROVIDER_SETUP_GUIDES[formType].map((step, i) => (
                    <li key={i} className="font-mono">
                      {step.replace(/\{public_url\}/g, publicUrl || 'https://your-dashboard-url')}
                    </li>
                  ))}
                </ol>
              </div>
            )}

            {(PROVIDER_FIELDS[formType] || []).map(f => (
              <div key={f.key} className="mb-3">
                <label className="block text-xs text-gray-500 mb-1">
                  {f.label}
                  {f.required && <span className="text-red-400 ml-0.5">*</span>}
                </label>
                <input
                  type={f.secret ? 'password' : 'text'}
                  value={formFields[f.key] || ''}
                  onChange={e => setFormFields(prev => ({ ...prev, [f.key]: e.target.value }))}
                  placeholder={f.secret && editingId ? '\u2022\u2022\u2022\u2022\u2022\u2022\u2022\u2022 (leave empty to keep)' : ''}
                  className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none font-mono"
                />
                {f.help && <p className="text-[10px] text-gray-600 mt-0.5">{f.help}</p>}
              </div>
            ))}

            <div className="flex items-center gap-2 mb-4">
              <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
                <input
                  type="checkbox"
                  checked={formEnabled}
                  onChange={e => setFormEnabled(e.target.checked)}
                  className="accent-cyan-500"
                />
                Enabled
              </label>
            </div>

            <div className="flex items-center gap-2">
              <button
                onClick={handleSave}
                disabled={saving}
                className="px-4 py-1.5 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white transition-colors disabled:opacity-50"
              >
                {saving ? 'Saving...' : editingId ? 'Update' : 'Create'}
              </button>
              <button
                onClick={resetForm}
                className="px-4 py-1.5 text-xs rounded border border-gray-700 text-gray-400 hover:text-gray-200 transition-colors"
              >
                Cancel
              </button>
            </div>
          </div>
        )}
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
        <button onClick={() => setTab('auth')} className={tabCls('auth')}>Auth</button>
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

      {tab === 'auth' && <AuthTab />}
    </div>
  );
}
