import { useState, useCallback } from 'react';
import { api, type DashUser } from '../api/client';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';

const roleBadge: Record<string, string> = {
  admin: 'bg-green-500/20 text-green-400',
  operator: 'bg-cyan-500/20 text-cyan-400',
  viewer: 'bg-gray-500/20 text-gray-400',
};

const providerBadge: Record<string, string> = {
  local: 'bg-gray-700 text-gray-300',
  microsoft: 'bg-blue-500/20 text-blue-400',
  google: 'bg-red-500/20 text-red-400',
};

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

type Tab = 'pending' | 'all';

export function UsersPage() {
  usePageTitle('Users');
  const toast = useToast();

  const [tab, setTab] = useState<Tab>('all');
  const [allUsers, setAllUsers] = useState<DashUser[]>([]);
  const [pendingUsers, setPendingUsers] = useState<DashUser[]>([]);
  const [pendingCount, setPendingCount] = useState(0);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [pendingRoles, setPendingRoles] = useState<Record<string, string>>({});
  const [changeRoles, setChangeRoles] = useState<Record<string, string>>({});
  const [showInvite, setShowInvite] = useState(false);
  const [inviteEmail, setInviteEmail] = useState('');
  const [inviteRole, setInviteRole] = useState('viewer');
  const [inviteLoading, setInviteLoading] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [all, pending] = await Promise.all([
        api.getUsers(),
        api.getPendingUsers(),
      ]);
      setAllUsers(all);
      setPendingUsers(pending.users);
      setPendingCount(pending.count);
    } catch {
      // silently retry on next poll
    }
  }, []);

  usePolling(refresh, 10000);

  const handleApprove = async (userId: string) => {
    const role = pendingRoles[userId] || 'viewer';
    try {
      await api.approveUser(userId, role);
      toast('success', 'User approved');
      refresh();
    } catch {
      toast('error', 'Failed to approve user');
    }
  };

  const handleDeny = async (userId: string) => {
    try {
      await api.denyUser(userId);
      toast('success', 'User denied');
      refresh();
    } catch {
      toast('error', 'Failed to deny user');
    }
  };

  const handleSetRole = async (userId: string) => {
    const role = changeRoles[userId];
    if (!role) return;
    try {
      await api.setUserRole(userId, role);
      toast('success', 'Role updated');
      setExpandedId(null);
      refresh();
    } catch {
      toast('error', 'Failed to update role');
    }
  };

  const handleDisable = async (userId: string) => {
    try {
      await api.disableUser(userId);
      toast('success', 'User disabled');
      setExpandedId(null);
      refresh();
    } catch {
      toast('error', 'Failed to disable user');
    }
  };

  const handleInvite = async () => {
    if (!inviteEmail.trim()) return;
    setInviteLoading(true);
    try {
      await api.inviteUser(inviteEmail.trim(), inviteRole);
      toast('success', `Invited ${inviteEmail.trim()}`);
      setInviteEmail('');
      setInviteRole('viewer');
      setShowInvite(false);
      refresh();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to invite user';
      toast('error', msg.includes('409') || msg.includes('Conflict') ? 'Email already registered' : msg);
    } finally {
      setInviteLoading(false);
    }
  };

  const activeUsers = allUsers.filter((u) => u.status === 'active');
  const disabledUsers = allUsers.filter((u) => u.status === 'disabled' || u.status === 'denied');

  return (
    <div className="p-4 md:p-6 max-w-4xl">
      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-lg md:text-xl font-bold text-gray-100">Users</h1>
        <button
          onClick={() => setShowInvite(!showInvite)}
          className="px-3 py-1.5 text-xs rounded border border-cyan-700 text-cyan-400 hover:bg-cyan-500/10 transition-colors"
        >
          Invite
        </button>
      </div>

      {/* Invite form */}
      {showInvite && (
        <div className="mb-4 border border-gray-800 rounded bg-[var(--bg-card)] p-3">
          <div className="flex items-end gap-2 flex-wrap">
            <div className="flex-1 min-w-[200px]">
              <label className="block text-xs text-gray-500 mb-1">Email</label>
              <input
                type="email"
                value={inviteEmail}
                onChange={(e) => setInviteEmail(e.target.value)}
                placeholder="user@company.com"
                className="w-full bg-transparent border-b border-gray-700 focus:border-cyan-500/50 py-1.5 text-sm text-gray-200 focus:outline-none placeholder:text-gray-700 font-mono"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-500 mb-1">Role</label>
              <select
                value={inviteRole}
                onChange={(e) => setInviteRole(e.target.value)}
                className="text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1.5 text-gray-300"
              >
                <option value="viewer">viewer</option>
                <option value="operator">operator</option>
                <option value="admin">admin</option>
              </select>
            </div>
            <button
              onClick={handleInvite}
              disabled={inviteLoading || !inviteEmail.trim()}
              className="px-3 py-1.5 text-xs rounded bg-cyan-500/20 text-cyan-400 hover:bg-cyan-500/30 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
            >
              {inviteLoading ? 'Sending...' : 'Send Invite'}
            </button>
            <button
              onClick={() => { setShowInvite(false); setInviteEmail(''); }}
              className="px-2 py-1.5 text-xs text-gray-500 hover:text-gray-300 transition-colors"
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-gray-800">
        <button
          onClick={() => setTab('pending')}
          className={`px-3 py-2 text-sm border-b-2 transition-colors ${
            tab === 'pending'
              ? 'border-yellow-500 text-yellow-400'
              : 'border-transparent text-gray-500 hover:text-gray-300'
          }`}
        >
          Pending{pendingCount > 0 ? ` (${pendingCount})` : ''}
        </button>
        <button
          onClick={() => setTab('all')}
          className={`px-3 py-2 text-sm border-b-2 transition-colors ${
            tab === 'all'
              ? 'border-cyan-500 text-cyan-400'
              : 'border-transparent text-gray-500 hover:text-gray-300'
          }`}
        >
          All ({allUsers.length})
        </button>
      </div>

      {/* Pending tab */}
      {tab === 'pending' && (
        <div className="space-y-2">
          {pendingUsers.length === 0 && (
            <p className="text-gray-500 text-sm py-8 text-center">No pending users</p>
          )}
          {pendingUsers.map((u) => (
            <div
              key={u.user_id}
              className="border border-gray-800 border-l-3 border-l-yellow-500 rounded bg-[var(--bg-card)] p-3"
            >
              <div className="flex items-center justify-between gap-2 flex-wrap">
                <div className="flex items-center gap-2 min-w-0">
                  <span className="text-sm text-gray-100 truncate font-mono">{u.email}</span>
                  <span className={`text-[10px] px-1.5 py-0.5 rounded ${providerBadge[u.auth_provider] || providerBadge.local}`}>
                    {u.auth_provider}
                  </span>
                  <span className="text-xs text-gray-600">{timeAgo(u.created_at)}</span>
                </div>
                <div className="flex items-center gap-2">
                  <select
                    value={pendingRoles[u.user_id] || 'viewer'}
                    onChange={(e) => setPendingRoles((prev) => ({ ...prev, [u.user_id]: e.target.value }))}
                    className="text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1 text-gray-300"
                  >
                    <option value="viewer">viewer</option>
                    <option value="operator">operator</option>
                    <option value="admin">admin</option>
                  </select>
                  <button
                    onClick={() => handleApprove(u.user_id)}
                    className="px-2 py-1 text-xs rounded bg-green-500/20 text-green-400 hover:bg-green-500/30 transition-colors"
                  >
                    Approve
                  </button>
                  <button
                    onClick={() => handleDeny(u.user_id)}
                    className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors"
                    title="Deny"
                  >
                    &#10005;
                  </button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* All tab */}
      {tab === 'all' && (
        <div className="space-y-2">
          {/* Active users */}
          {activeUsers.map((u) => {
            const expanded = expandedId === u.user_id;
            return (
              <div
                key={u.user_id}
                className="border border-gray-800 rounded bg-[var(--bg-card)] p-3 cursor-pointer hover:border-gray-700 transition-colors"
                onClick={() => setExpandedId(expanded ? null : u.user_id)}
              >
                <div className="flex items-center justify-between gap-2 flex-wrap">
                  <div className="flex items-center gap-2 min-w-0">
                    <span className="text-sm text-gray-100 truncate font-mono">{u.email}</span>
                    <span className={`text-[10px] px-1.5 py-0.5 rounded ${providerBadge[u.auth_provider] || providerBadge.local}`}>
                      {u.auth_provider}
                    </span>
                  </div>
                  <div className="flex items-center gap-2">
                    {u.last_login_at && (
                      <span className="text-xs text-gray-600">{timeAgo(u.last_login_at)}</span>
                    )}
                    <span className={`text-[10px] px-1.5 py-0.5 rounded font-mono ${roleBadge[u.role] || roleBadge.viewer}`}>
                      {u.role}
                    </span>
                  </div>
                </div>
                {expanded && (
                  <div
                    className="mt-3 pt-3 border-t border-gray-800 flex items-center gap-3 flex-wrap"
                    onClick={(e) => e.stopPropagation()}
                  >
                    <label className="text-xs text-gray-500">Change role:</label>
                    <select
                      value={changeRoles[u.user_id] || u.role}
                      onChange={(e) => setChangeRoles((prev) => ({ ...prev, [u.user_id]: e.target.value }))}
                      className="text-xs bg-gray-800 border border-gray-700 rounded px-2 py-1 text-gray-300"
                    >
                      <option value="viewer">viewer</option>
                      <option value="operator">operator</option>
                      <option value="admin">admin</option>
                    </select>
                    <button
                      onClick={() => handleSetRole(u.user_id)}
                      disabled={!changeRoles[u.user_id] || changeRoles[u.user_id] === u.role}
                      className="px-2 py-1 text-xs rounded bg-cyan-500/20 text-cyan-400 hover:bg-cyan-500/30 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
                    >
                      Save
                    </button>
                    <button
                      onClick={() => handleDisable(u.user_id)}
                      className="px-2 py-1 text-xs rounded text-red-400 hover:bg-red-500/20 transition-colors ml-auto"
                    >
                      Disable
                    </button>
                  </div>
                )}
              </div>
            );
          })}

          {/* Disabled / denied users */}
          {disabledUsers.length > 0 && (
            <>
              <div className="text-xs text-gray-600 mt-4 mb-1 uppercase tracking-wide">Inactive</div>
              {disabledUsers.map((u) => (
                <div
                  key={u.user_id}
                  className="border border-gray-800/50 rounded bg-[var(--bg-card)] p-3 opacity-50"
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm text-gray-400 truncate font-mono">{u.email}</span>
                    <span className="text-[10px] px-1.5 py-0.5 rounded bg-red-500/20 text-red-400">
                      {u.status}
                    </span>
                  </div>
                </div>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}
