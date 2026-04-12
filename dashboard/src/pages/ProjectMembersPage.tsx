import { useState, useEffect, useCallback, useRef } from 'react';
import { api } from '../api/client';
import type { ProjectMember, WorkspaceInvite, ImportResult } from '../api/types';
import { useProject } from '../hooks/useProject';
import { useAuthStore } from '../stores/authStore';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { RoleBadge } from '../components/common/RoleBadge';
import { PageHeader } from '../components/common/PageHeader';
import { EmptyState } from '../components/common/EmptyState';
import { SettingsTabs } from '../components/common/SettingsTabs';

const ROLES = ['admin', 'operator', 'viewer'] as const;
type StatusFilter = 'all' | 'active' | 'pending_acceptance' | 'denied';

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

function StatusBadge({ status }: { status: string }) {
  switch (status) {
    case 'active':
      return <span className="inline-block px-1.5 py-0.5 text-xs rounded text-green-400 bg-green-900/20">active</span>;
    case 'pending_acceptance':
      return <span className="inline-block px-1.5 py-0.5 text-xs rounded text-yellow-400 bg-yellow-900/20">pending</span>;
    case 'denied':
      return <span className="inline-block px-1.5 py-0.5 text-xs rounded text-red-400 bg-red-900/20">denied</span>;
    default:
      return <span className="inline-block px-1.5 py-0.5 text-xs rounded text-gray-400 bg-gray-900/20">{status}</span>;
  }
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

  // New state for upgraded features
  const [statusFilter, setStatusFilter] = useState<StatusFilter>('all');
  const [selectedPending, setSelectedPending] = useState<Set<string>>(new Set());
  const [sendingInvites, setSendingInvites] = useState(false);
  const [showImport, setShowImport] = useState(false);
  const [importFile, setImportFile] = useState<File | null>(null);
  const [importPreview, setImportPreview] = useState<string[][] | null>(null);
  const [importing, setImporting] = useState(false);
  const [importResult, setImportResult] = useState<ImportResult | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

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

  // CSV import handlers
  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setImportFile(file);
    setImportResult(null);

    const reader = new FileReader();
    reader.onload = (ev) => {
      const text = ev.target?.result as string;
      const lines = text.trim().split('\n').map(line =>
        line.split(',').map(cell => cell.trim().replace(/^"|"$/g, ''))
      );
      setImportPreview(lines.slice(0, 10)); // show up to 10 rows
    };
    reader.readAsText(file);
  };

  const handleImport = async () => {
    if (!projectId || !importFile) return;
    setImporting(true);
    try {
      const result = await api.importMembers(projectId, importFile);
      setImportResult(result);
      addToast('success', `Imported ${result.imported}, skipped ${result.skipped}, errors ${result.errors}`);
    } catch {
      addToast('error', 'Failed to import members');
    } finally {
      setImporting(false);
    }
  };

  const handleCloseImport = () => {
    setShowImport(false);
    setImportFile(null);
    setImportPreview(null);
    setImportResult(null);
    if (fileInputRef.current) fileInputRef.current.value = '';
    loadData();
  };

  // Bulk send invites
  const handleBulkSendInvites = async () => {
    if (!projectId || selectedPending.size === 0) return;
    setSendingInvites(true);
    try {
      const result = await api.sendInvites(projectId, Array.from(selectedPending));
      addToast('success', `Sent ${result.sent} invite(s)`);
      setSelectedPending(new Set());
      loadData();
    } catch {
      addToast('error', 'Failed to send invites');
    } finally {
      setSendingInvites(false);
    }
  };

  // Single send invite
  const handleSendInvite = async (userId: string) => {
    if (!projectId) return;
    try {
      const result = await api.sendInvites(projectId, [userId]);
      addToast('success', `Sent ${result.sent} invite(s)`);
      loadData();
    } catch {
      addToast('error', 'Failed to send invite');
    }
  };

  const togglePendingSelection = (userId: string) => {
    setSelectedPending(prev => {
      const next = new Set(prev);
      if (next.has(userId)) next.delete(userId);
      else next.add(userId);
      return next;
    });
  };

  const toggleAllPending = () => {
    const pendingMembers = filteredMembers.filter(m => m.status === 'pending_acceptance');
    if (selectedPending.size === pendingMembers.length && pendingMembers.length > 0) {
      setSelectedPending(new Set());
    } else {
      setSelectedPending(new Set(pendingMembers.map(m => m.user_id)));
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
  const filteredMembers = statusFilter === 'all'
    ? members
    : members.filter(m => m.status === statusFilter);
  const hasPendingInFilter = filteredMembers.some(m => m.status === 'pending_acceptance');

  return (
    <div className="p-4 md:p-6">
      <PageHeader
        title="Settings"
        action={
          <div className="flex gap-2">
            <button
              onClick={() => setShowImport(true)}
              className="px-3 py-1.5 text-sm border border-gray-700 text-gray-300 hover:text-gray-100 hover:border-gray-600 rounded transition-colors"
            >
              Import CSV
            </button>
            <button onClick={() => { setShowInvite(true); setShowAddExisting(false); setInviteUrl(null); }} className="btn-primary">
              Invite
            </button>
          </div>
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

      {/* Filter bar + bulk actions */}
      <div className="flex items-center gap-3 mb-3">
        <label className="text-xs text-gray-500">Filter:</label>
        <div className="flex gap-1">
          {([
            ['all', 'All'],
            ['active', 'Active'],
            ['pending_acceptance', 'Pending'],
            ['denied', 'Denied'],
          ] as const).map(([value, label]) => (
            <button
              key={value}
              onClick={() => { setStatusFilter(value); setSelectedPending(new Set()); }}
              className={`px-2 py-1 text-xs rounded border transition-colors ${
                statusFilter === value
                  ? 'border-cyan-700 text-cyan-400 bg-cyan-900/20'
                  : 'border-gray-800 text-gray-500 hover:text-gray-300 hover:border-gray-700'
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        {selectedPending.size > 0 && (
          <button
            onClick={handleBulkSendInvites}
            disabled={sendingInvites}
            className="ml-auto px-3 py-1 text-xs border border-cyan-800 text-cyan-400 hover:bg-cyan-900/20 rounded transition-colors disabled:opacity-50"
          >
            {sendingInvites ? 'Sending...' : `Send Invite Email (${selectedPending.size})`}
          </button>
        )}
      </div>

      {/* Members table */}
      {filteredMembers.length === 0 ? (
        <EmptyState message={statusFilter === 'all' ? 'No members yet' : `No ${statusFilter === 'pending_acceptance' ? 'pending' : statusFilter} members`} />
      ) : (
        <div className="table-container">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
                {hasPendingInFilter && (
                  <th className="px-2 py-2.5 text-center font-medium w-8">
                    <input
                      type="checkbox"
                      checked={selectedPending.size > 0 && selectedPending.size === filteredMembers.filter(m => m.status === 'pending_acceptance').length}
                      onChange={toggleAllPending}
                      className="accent-cyan-500"
                    />
                  </th>
                )}
                <th className="px-4 py-2.5 text-left font-medium">Email</th>
                <th className="px-4 py-2.5 text-left font-medium">Display Name</th>
                <th className="px-4 py-2.5 text-left font-medium">Role</th>
                <th className="px-4 py-2.5 text-left font-medium">Status</th>
                <th className="px-4 py-2.5 text-left font-medium">Joined</th>
                <th className="px-4 py-2.5 text-left font-medium"></th>
              </tr>
            </thead>
            <tbody>
              {filteredMembers.map(member => {
                const isSelf = member.email === currentEmail;
                const isPending = member.status === 'pending_acceptance';
                return (
                  <tr key={member.user_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
                    {hasPendingInFilter && (
                      <td className="px-2 py-3 text-center">
                        {isPending ? (
                          <input
                            type="checkbox"
                            checked={selectedPending.has(member.user_id)}
                            onChange={() => togglePendingSelection(member.user_id)}
                            className="accent-cyan-500"
                          />
                        ) : null}
                      </td>
                    )}
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
                    <td className="px-4 py-3">
                      <StatusBadge status={member.status} />
                    </td>
                    <td className="px-4 py-3 text-gray-500 text-xs">
                      {new Date(member.joined_at).toLocaleDateString()}
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex items-center gap-2">
                        {isPending && (
                          <button
                            onClick={() => handleSendInvite(member.user_id)}
                            className="text-xs text-cyan-600 hover:text-cyan-400 transition-colors"
                          >
                            {member.invite_sent_at ? 'Resend' : 'Send'}
                          </button>
                        )}
                        {!isSelf && (
                          <button
                            onClick={() => handleRemove(member.user_id, member.email)}
                            className="text-xs text-gray-600 hover:text-red-400 transition-colors"
                          >
                            Remove
                          </button>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {/* CSV Import Modal */}
      {showImport && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
          <div className="bg-[var(--bg-base,#0d1117)] border border-gray-800 rounded-lg w-full max-w-lg mx-4 p-5">
            <h3 className="text-sm text-gray-200 font-medium mb-4">Import Members from CSV</h3>

            {!importResult ? (
              <>
                <div className="mb-4">
                  <label className="block text-xs text-gray-400 mb-2">
                    CSV file with columns: email, role (optional, defaults to viewer)
                  </label>
                  <input
                    ref={fileInputRef}
                    type="file"
                    accept=".csv"
                    onChange={handleFileSelect}
                    className="block w-full text-xs text-gray-400 file:mr-3 file:py-1.5 file:px-3 file:rounded file:border file:border-gray-700 file:text-xs file:text-gray-300 file:bg-transparent hover:file:border-gray-600 file:cursor-pointer"
                  />
                </div>

                {importPreview && (
                  <div className="mb-4">
                    <p className="text-xs text-gray-500 mb-1">Preview (first {importPreview.length} rows):</p>
                    <div className="border border-gray-800 rounded overflow-auto max-h-40">
                      <table className="w-full text-xs">
                        <tbody>
                          {importPreview.map((row, i) => (
                            <tr key={i} className={i === 0 ? 'text-gray-400 bg-gray-900/50' : 'text-gray-300'}>
                              {row.map((cell, j) => (
                                <td key={j} className="px-2 py-1 border-b border-gray-800/50 font-mono">{cell}</td>
                              ))}
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </div>
                )}

                <div className="flex gap-2 justify-end">
                  <button
                    onClick={handleCloseImport}
                    className="text-gray-400 hover:text-gray-200 px-3 py-1.5 text-sm"
                  >
                    Cancel
                  </button>
                  <button
                    onClick={handleImport}
                    disabled={!importFile || importing}
                    className="btn-primary disabled:opacity-50"
                  >
                    {importing ? 'Importing...' : 'Import'}
                  </button>
                </div>
              </>
            ) : (
              <>
                <div className="mb-4 space-y-2">
                  <div className="flex gap-4 text-xs">
                    <span className="text-green-400">Imported: {importResult.imported}</span>
                    <span className="text-yellow-400">Skipped: {importResult.skipped}</span>
                    <span className="text-red-400">Errors: {importResult.errors}</span>
                  </div>
                  {importResult.details.length > 0 && (
                    <div className="border border-gray-800 rounded overflow-auto max-h-48">
                      <table className="w-full text-xs">
                        <thead>
                          <tr className="text-gray-500 bg-gray-900/50">
                            <th className="px-2 py-1 text-left font-medium">Email</th>
                            <th className="px-2 py-1 text-left font-medium">Result</th>
                            <th className="px-2 py-1 text-left font-medium">Message</th>
                          </tr>
                        </thead>
                        <tbody>
                          {importResult.details.map((d, i) => (
                            <tr key={i} className="border-t border-gray-800/50">
                              <td className="px-2 py-1 text-gray-300 font-mono">{d.email}</td>
                              <td className="px-2 py-1">
                                <span className={
                                  d.result === 'imported' ? 'text-green-400' :
                                  d.result === 'skipped' ? 'text-yellow-400' :
                                  'text-red-400'
                                }>{d.result}</span>
                              </td>
                              <td className="px-2 py-1 text-gray-500">{d.message}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  )}
                </div>
                <div className="flex justify-end">
                  <button onClick={handleCloseImport} className="btn-primary">
                    Close
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
