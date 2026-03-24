import { useState, useEffect, useCallback } from 'react';
import { api } from '../api/client';
import type { ProjectMember } from '../api/types';
import { useProject } from '../hooks/useProject';
import { useAuthStore } from '../stores/authStore';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { RoleBadge } from '../components/common/RoleBadge';

const ROLES = ['admin', 'operator', 'viewer'] as const;

export function ProjectMembersPage() {
  const { projectId } = useProject();
  const currentEmail = useAuthStore(s => s.email);
  const [members, setMembers] = useState<ProjectMember[]>([]);
  const [loading, setLoading] = useState(true);
  const [showAdd, setShowAdd] = useState(false);
  const [newEmail, setNewEmail] = useState('');
  const [newRole, setNewRole] = useState<string>('viewer');
  const [adding, setAdding] = useState(false);
  const addToast = useToast();

  usePageTitle('Workspace Members');

  const loadMembers = useCallback(async () => {
    if (!projectId) return;
    try {
      const data = await api.getProjectMembers(projectId);
      setMembers(data);
    } catch {
      addToast('error', 'Failed to load members');
    } finally {
      setLoading(false);
    }
  }, [projectId, addToast]);

  useEffect(() => { loadMembers(); }, [loadMembers]);

  const handleAdd = async () => {
    if (!projectId || !newEmail.trim()) return;
    setAdding(true);
    try {
      await api.addProjectMember(projectId, newEmail.trim(), newRole);
      addToast('success', `Added ${newEmail.trim()}`);
      setShowAdd(false);
      setNewEmail('');
      setNewRole('viewer');
      loadMembers();
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
      loadMembers();
    } catch {
      addToast('error', 'Failed to update role');
    }
  };

  const handleRemove = async (userId: string, email: string) => {
    if (!projectId) return;
    try {
      await api.removeProjectMember(projectId, userId);
      addToast('success', `Removed ${email}`);
      loadMembers();
    } catch {
      addToast('error', 'Failed to remove member');
    }
  };

  if (loading) {
    return (
      <div className="p-4 md:p-6">
        <h2 className="text-xl font-bold text-gray-100 mb-6">Workspace Members</h2>
        <p className="text-gray-500 motion-safe:animate-pulse">Loading members...</p>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-bold text-gray-100">Workspace Members</h2>
        <button
          onClick={() => setShowAdd(true)}
          className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-2 rounded text-sm transition-colors"
        >
          Add Member
        </button>
      </div>

      {showAdd && (
        <div className="border border-gray-800 rounded p-4 mb-6">
          <h3 className="text-sm text-gray-200 font-medium mb-3">Add Member</h3>
          <div className="flex gap-3 items-end">
            <div className="flex-1">
              <label className="block text-xs text-gray-400 mb-1">Email</label>
              <input
                value={newEmail}
                onChange={e => setNewEmail(e.target.value)}
                placeholder="user@company.com"
                className="w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                autoFocus
              />
            </div>
            <div>
              <label className="block text-xs text-gray-400 mb-1">Role</label>
              <select
                value={newRole}
                onChange={e => setNewRole(e.target.value)}
                className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              >
                {ROLES.map(r => (
                  <option key={r} value={r}>{r.charAt(0).toUpperCase() + r.slice(1)}</option>
                ))}
              </select>
            </div>
            <button
              onClick={handleAdd}
              disabled={adding || !newEmail.trim()}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
            >
              {adding ? 'Adding...' : 'Add'}
            </button>
            <button
              onClick={() => { setShowAdd(false); setNewEmail(''); }}
              className="text-gray-400 hover:text-gray-200 px-3 py-1.5 text-sm"
            >
              Cancel
            </button>
          </div>
        </div>
      )}

      {members.length === 0 ? (
        <div className="border border-gray-800 rounded p-8 text-center">
          <p className="text-gray-500 text-sm">No members yet</p>
        </div>
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
                          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 py-1 text-xs text-gray-200 focus:outline-none focus:border-cyan-500"
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
