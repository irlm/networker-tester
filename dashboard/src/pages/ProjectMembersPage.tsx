import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import type { ProjectMember, WorkspaceInvite } from '../api/types';
import { useProject } from '../hooks/useProject';
import { useAuthStore } from '../stores/authStore';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { RoleBadge } from '../components/common/RoleBadge';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { SettingsTabs } from '../components/common/SettingsTabs';

const ROLES = ['admin', 'operator', 'viewer'] as const;

function relativeTime(dateStr: string): string {
  const now = Date.now();
  const target = new Date(dateStr).getTime();
  const diff = target - now;
  const absDiff = Math.abs(diff);
  const minutes = Math.floor(absDiff / 60000);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);

  if (diff < 0) return 'expired';
  if (days > 0) return `${days}d`;
  if (hours > 0) return `${hours}h`;
  return `${minutes}m`;
}

export function ProjectMembersPage() {
  const { projectId } = useProject();
  const currentEmail = useAuthStore(s => s.email);
  const [members, setMembers] = useState<ProjectMember[]>([]);
  const [invites, setInvites] = useState<WorkspaceInvite[]>([]);
  const [loading, setLoading] = useState(true);
  const [showInvite, setShowInvite] = useState(false);
  const [showAddExisting, setShowAddExisting] = useState(false);
  const [newEmail, setNewEmail] = useState('');
  const [newRole, setNewRole] = useState<string>('viewer');
  const [adding, setAdding] = useState(false);
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const addToast = useToast();

  usePageTitle('Settings');

  const loadData = useCallback(async () => {
    if (!projectId) return;
    try {
      const [membersData, invitesData] = await Promise.all([
        api.getProjectMembers(projectId),
        api.getInvites(projectId).catch(() => [] as WorkspaceInvite[]),
      ]);
      setMembers(membersData);
      setInvites(invitesData);
    } catch {
      addToast('error', 'Failed to load members');
    } finally {
      setLoading(false);
    }
  }, [projectId, addToast]);

  useEffect(() => { loadData(); }, [loadData]);

  const handleInvite = async () => {
    if (!projectId || !newEmail.trim()) return;
    setAdding(true);
    try {
      const res = await api.createInvite(projectId, newEmail.trim(), newRole);
      addToast('success', `Invite sent to ${newEmail.trim()}`);
      setInviteUrl(res.url);
      setNewEmail('');
      setNewRole('viewer');
      loadData();
    } catch {
      addToast('error', 'Failed to send invite');
    } finally {
      setAdding(false);
    }
  };

  const handleCopyUrl = async () => {
    if (!inviteUrl) return;
    try {
      await navigator.clipboard.writeText(inviteUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      addToast('error', 'Failed to copy');
    }
  };

  const handleRevokeInvite = async (inviteId: string, email: string) => {
    if (!projectId) return;
    try {
      await api.revokeInvite(projectId, inviteId);
      addToast('success', `Revoked invite for ${email}`);
      loadData();
    } catch {
      addToast('error', 'Failed to revoke invite');
    }
  };

  const handleAddExisting = async () => {
    if (!projectId || !newEmail.trim()) return;
    setAdding(true);
    try {
      await api.addProjectMember(projectId, newEmail.trim(), newRole);
      addToast('success', `Added ${newEmail.trim()}`);
      setShowAddExisting(false);
      setNewEmail('');
      setNewRole('viewer');
      loadData();
    } catch {
      addToast('error', 'Failed to add member');
    } finally {
      setAdding(false);
    }
  };

  const handleRoleChange = async (userId: string, role: string) => {
    if (!projectId) return;
    try {
      await api.updateMemberRole(projectId, userId, role);
      addToast('success', 'Role updated');
      loadData();
    } catch {
      addToast('error', 'Failed to update role');
    }
  };

  const handleRemove = async (userId: string, email: string) => {
    if (!projectId) return;
    try {
      await api.removeProjectMember(projectId, userId);
      addToast('success', `Removed ${email}`);
      loadData();
    } catch {
      addToast('error', 'Failed to remove member');
    }
  };

  if (loading) {
    return (
      <div className="p-4 md:p-6">
        <PageHeader title="Settings" />
        <SettingsTabs />
        <p className="text-gray-500 motion-safe:animate-pulse">Loading members...</p>
      </div>
    );
  }

  const pendingInvites = invites.filter(i => i.status === 'pending');

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Settings"
        action={
          <button onClick={() => { setShowInvite(true); setShowAddExisting(false); setInviteUrl(null); }} className="btn-primary">
            Invite
          </button>
        }
      />
      <SettingsTabs />

      {/* Invite form */}
      {showInvite && (
        <div className="border border-gray-800 rounded p-4 mb-6">
          <h3 className="text-sm text-gray-200 font-medium mb-3">Invite to Workspace</h3>
          <div className="flex gap-3 items-end">
            <div className="flex-1">
              <label className="block text-xs text-gray-400 mb-1">Email</label>
              <input
                value={newEmail}
                onChange={e => setNewEmail(e.target.value)}
                placeholder="user@company.com"
                className="input"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Role</label>
              <select value={newRole} onChange={e => setNewRole(e.target.value)} className="input">
                {ROLES.map(r => (
                  <option key={r} value={r}>{r.charAt(0).toUpperCase() + r.slice(1)}</option>
                ))}
              </select>
            </div>
            <button onClick={handleInvite} disabled={adding || !newEmail.trim()} className="btn-primary">
              {adding ? 'Sending...' : 'Send Invite'}
            </button>
            <button
              onClick={() => { setShowInvite(false); setNewEmail(''); setInviteUrl(null); }}
              className="text-gray-400 hover:text-gray-200 px-3 py-1.5 text-sm"
            >
              Cancel
            </button>
          </div>

          {/* Invite URL copy section */}
          {inviteUrl && (
            <div className="mt-3 flex items-center gap-2 bg-gray-900/50 border border-gray-800 rounded px-3 py-2">
              <span className="text-xs text-gray-500 shrink-0">Invite link:</span>
              <code className="text-xs text-gray-300 truncate flex-1">{inviteUrl}</code>
              <button
                onClick={handleCopyUrl}
                className="text-xs text-cyan-400 hover:text-cyan-300 shrink-0 transition-colors"
              >
                {copied ? 'Copied' : 'Copy'}
              </button>
            </div>
          )}

          <div className="mt-2">
            <button
              onClick={() => { setShowInvite(false); setShowAddExisting(true); setInviteUrl(null); }}
              className="text-xs text-gray-600 hover:text-gray-400 transition-colors"
            >
              or add existing user directly
            </button>
          </div>
        </div>
      )}

      {/* Add existing user form (secondary) */}
      {showAddExisting && (
        <div className="border border-gray-800 rounded p-4 mb-6">
          <h3 className="text-sm text-gray-200 font-medium mb-3">Add Existing User</h3>
          <div className="flex gap-3 items-end">
            <div className="flex-1">
              <label className="block text-xs text-gray-400 mb-1">Email</label>
              <input
                value={newEmail}
                onChange={e => setNewEmail(e.target.value)}
                placeholder="user@company.com"
                className="input"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Role</label>
              <select value={newRole} onChange={e => setNewRole(e.target.value)} className="input">
                {ROLES.map(r => (
                  <option key={r} value={r}>{r.charAt(0).toUpperCase() + r.slice(1)}</option>
                ))}
              </select>
            </div>
            <button onClick={handleAddExisting} disabled={adding || !newEmail.trim()} className="btn-primary">
              {adding ? 'Adding...' : 'Add'}
            </button>
            <button
              onClick={() => { setShowAddExisting(false); setNewEmail(''); }}
              className="text-gray-400 hover:text-gray-200 px-3 py-1.5 text-sm"
            >
              Cancel
            </button>
          </div>
          <div className="mt-2">
            <button
              onClick={() => { setShowAddExisting(false); setShowInvite(true); }}
              className="text-xs text-gray-600 hover:text-gray-400 transition-colors"
            >
              or send an invite link instead
            </button>
          </div>
        </div>
      )}

      {/* Pending invites */}
      {pendingInvites.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-500 uppercase tracking-wider mb-2">Pending Invites</h3>
          <div className="table-container">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                  <th className="px-4 py-2.5 text-left font-medium">Email</th>
                  <th className="px-4 py-2.5 text-left font-medium">Role</th>
                  <th className="px-4 py-2.5 text-left font-medium">Invited By</th>
                  <th className="px-4 py-2.5 text-left font-medium">Expires</th>
                  <th className="px-4 py-2.5 text-left font-medium"></th>
                </tr>
              </thead>
              <tbody>
                {pendingInvites.map(invite => (
                  <tr key={invite.invite_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                    <td className="px-4 py-3 text-gray-200">{invite.email}</td>
                    <td className="px-4 py-3">
                      <RoleBadge role={invite.role} className="text-xs" />
                    </td>
                    <td className="px-4 py-3 text-gray-400 text-xs">{invite.invited_by_email}</td>
                    <td className="px-4 py-3 text-gray-500 text-xs">{relativeTime(invite.expires_at)}</td>
                    <td className="px-4 py-3">
                      <button
                        onClick={() => handleRevokeInvite(invite.invite_id, invite.email)}
                        className="text-xs text-gray-600 hover:text-red-400 transition-colors"
                      >
                        Revoke
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Members table */}
      {members.length === 0 ? (
        <EmptyState message="No members yet" />
      ) : (
        <div className="table-container">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                <th className="px-4 py-2.5 text-left font-medium">Email</th>
                <th className="px-4 py-2.5 text-left font-medium">Display Name</th>
                <th className="px-4 py-2.5 text-left font-medium">Role</th>
                <th className="px-4 py-2.5 text-left font-medium">Joined</th>
                <th className="px-4 py-2.5 text-left font-medium"></th>
              </tr>
            </thead>
            <tbody>
              {members.map(member => {
                const isSelf = member.email === currentEmail;
                return (
                  <tr key={member.user_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                    <td className="px-4 py-3 text-gray-200">{member.email}</td>
                    <td className="px-4 py-3 text-gray-400">{member.display_name || '\u2014'}</td>
                    <td className="px-4 py-3">
                      {isSelf ? (
                        <RoleBadge role={member.role} className="text-xs" />
                      ) : (
                        <select
                          value={member.role}
                          onChange={e => handleRoleChange(member.user_id, e.target.value)}
                          className="input !w-auto !px-2 !py-1 !text-xs"
                        >
                          {ROLES.map(r => (
                            <option key={r} value={r}>{r.charAt(0).toUpperCase() + r.slice(1)}</option>
                          ))}
                        </select>
                      )}
                    </td>
                    <td className="px-4 py-3 text-gray-500 text-xs">
                      {new Date(member.joined_at).toLocaleDateString()}
                    </td>
                    <td className="px-4 py-3">
                      {!isSelf && (
                        <button
                          onClick={() => handleRemove(member.user_id, member.email)}
                          className="text-xs text-gray-600 hover:text-red-400 transition-colors"
                        >
                          Remove
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
