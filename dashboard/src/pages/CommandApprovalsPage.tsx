import { useState, useEffect, useCallback } from 'react';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { useApprovalSSE } from '../hooks/useSSE';
import { api, type CommandApproval } from '../api/client';
import { SettingsTabs } from '../components/common/SettingsTabs';

type Tab = 'pending' | 'history';

const statusBadge: Record<string, string> = {
  pending: 'bg-yellow-500/20 text-yellow-400 border border-yellow-500/30',
  approved: 'bg-green-500/20 text-green-400 border border-green-500/30',
  denied: 'bg-red-500/20 text-red-400 border border-red-500/30',
  expired: 'bg-gray-500/20 text-gray-400 border border-gray-500/30',
};

function computeRelativeTime(iso: string): string {
  const d = new Date(iso);
  const diff = Date.now() - d.getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return d.toLocaleDateString();
}

function computeTimeUntil(iso: string): string | null {
  const d = new Date(iso);
  const diff = d.getTime() - Date.now();
  if (diff <= 0) return null;
  const mins = Math.floor(diff / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

function RelativeTime({ iso }: { iso: string }) {
  const [label, setLabel] = useState(() => computeRelativeTime(iso));
  useEffect(() => { setLabel(computeRelativeTime(iso)); }, [iso]);
  return <span>{label}</span>;
}

function TimeUntil({ iso }: { iso: string }) {
  const [label, setLabel] = useState(() => computeTimeUntil(iso));
  useEffect(() => { setLabel(computeTimeUntil(iso)); }, [iso]);
  if (label === null) return <span className="text-red-400">expired</span>;
  return <span>{label}</span>;
}

export function CommandApprovalsPage() {
  usePageTitle('Settings');
  const { projectId } = useProject();
  const [tab, setTab] = useState<Tab>('pending');
  const [pending, setPending] = useState<CommandApproval[]>([]);
  const [loading, setLoading] = useState(true);
  const [deciding, setDeciding] = useState<string | null>(null);
  const [denyReasonFor, setDenyReasonFor] = useState<string | null>(null);
  const [denyReason, setDenyReason] = useState('');

  const fetchPending = useCallback(async () => {
    if (!projectId) return;
    try {
      const data = await api.getPendingApprovals(projectId);
      setPending(data);
    } catch {
      // ignore
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    fetchPending();
  }, [fetchPending]);

  // Refresh on SSE events
  useApprovalSSE(() => {
    fetchPending();
  });

  const handleApprove = async (approvalId: string) => {
    setDeciding(approvalId);
    try {
      await api.decideApproval(projectId, approvalId, true);
      setPending(prev => prev.filter(a => a.approval_id !== approvalId));
    } catch {
      // ignore
    } finally {
      setDeciding(null);
    }
  };

  const handleDeny = async (approvalId: string) => {
    if (denyReasonFor !== approvalId) {
      setDenyReasonFor(approvalId);
      setDenyReason('');
      return;
    }
    setDeciding(approvalId);
    try {
      await api.decideApproval(projectId, approvalId, false, denyReason || undefined);
      setPending(prev => prev.filter(a => a.approval_id !== approvalId));
      setDenyReasonFor(null);
      setDenyReason('');
    } catch {
      // ignore
    } finally {
      setDeciding(null);
    }
  };

  return (
    <div className="p-6 max-w-5xl">
      <h1 className="text-lg font-semibold text-gray-100 mb-4">Settings</h1>
      <SettingsTabs />

      {/* Tabs */}
      <div className="flex gap-1 mb-4 border-b border-gray-800 pb-px">
        {(['pending', 'history'] as Tab[]).map(t => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`px-4 py-2 text-sm capitalize transition-colors ${
              tab === t
                ? 'text-gray-100 border-b-2 border-cyan-500'
                : 'text-gray-500 hover:text-gray-300'
            }`}
          >
            {t === 'pending' ? `Pending (${pending.length})` : 'History'}
          </button>
        ))}
      </div>

      {loading ? (
        <div className="text-gray-500 text-sm py-8 text-center">Loading...</div>
      ) : tab === 'pending' ? (
        pending.length === 0 ? (
          <div className="text-gray-500 text-sm py-8 text-center">No pending approvals</div>
        ) : (
          <div className="space-y-2">
            {pending.map(a => (
              <div
                key={a.approval_id}
                className="bg-gray-900/50 border border-gray-800 rounded p-4"
              >
                <div className="flex items-start justify-between gap-4">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2 mb-1">
                      <span className="text-sm font-mono text-gray-200">{a.command_type}</span>
                      <span className={`text-[10px] px-1.5 py-0.5 rounded ${statusBadge[a.status]}`}>
                        {a.status}
                      </span>
                    </div>
                    <div className="text-xs text-gray-500 space-x-3">
                      <span>by {a.requested_by_email}</span>
                      <span><RelativeTime iso={a.requested_at} /></span>
                      <span>expires: <TimeUntil iso={a.expires_at} /></span>
                    </div>
                    {a.command_detail && Object.keys(a.command_detail).length > 0 && (
                      <pre className="text-[11px] text-gray-400 mt-2 bg-gray-950 rounded px-2 py-1 overflow-x-auto">
                        {JSON.stringify(a.command_detail, null, 2)}
                      </pre>
                    )}
                  </div>
                  <div className="flex items-center gap-2 shrink-0">
                    <button
                      onClick={() => handleApprove(a.approval_id)}
                      disabled={deciding === a.approval_id}
                      className="px-3 py-1.5 text-xs bg-green-600 hover:bg-green-500 text-white rounded transition-colors disabled:opacity-50"
                    >
                      Approve
                    </button>
                    <button
                      onClick={() => handleDeny(a.approval_id)}
                      disabled={deciding === a.approval_id}
                      className="px-3 py-1.5 text-xs bg-red-600 hover:bg-red-500 text-white rounded transition-colors disabled:opacity-50"
                    >
                      Deny
                    </button>
                  </div>
                </div>
                {denyReasonFor === a.approval_id && (
                  <div className="mt-3 flex gap-2">
                    <input
                      type="text"
                      value={denyReason}
                      onChange={e => setDenyReason(e.target.value)}
                      placeholder="Reason (optional)"
                      className="flex-1 bg-gray-950 border border-gray-700 rounded px-2 py-1 text-sm text-gray-200 placeholder-gray-600"
                      autoFocus
                      onKeyDown={e => { if (e.key === 'Enter') handleDeny(a.approval_id); }}
                    />
                    <button
                      onClick={() => handleDeny(a.approval_id)}
                      disabled={deciding === a.approval_id}
                      className="px-3 py-1 text-xs bg-red-600 hover:bg-red-500 text-white rounded disabled:opacity-50"
                    >
                      Confirm Deny
                    </button>
                    <button
                      onClick={() => { setDenyReasonFor(null); setDenyReason(''); }}
                      className="px-3 py-1 text-xs text-gray-400 hover:text-gray-200"
                    >
                      Cancel
                    </button>
                  </div>
                )}
              </div>
            ))}
          </div>
        )
      ) : (
        <HistoryTab projectId={projectId} />
      )}
    </div>
  );
}

function HistoryTab({ projectId }: { projectId: string }) {
  const [approvals, setApprovals] = useState<CommandApproval[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!projectId) return;
    // For history we re-use the pending endpoint — the backend only returns pending.
    // A full history endpoint can be added later; for now show pending as the list.
    api.getPendingApprovals(projectId)
      .then(setApprovals)
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [projectId]);

  // Refresh on SSE events
  useApprovalSSE(() => {
    if (!projectId) return;
    api.getPendingApprovals(projectId).then(setApprovals).catch(() => {});
  });

  if (loading) return <div className="text-gray-500 text-sm py-8 text-center">Loading...</div>;

  if (approvals.length === 0) {
    return <div className="text-gray-500 text-sm py-8 text-center">No approval history</div>;
  }

  return (
    <table className="w-full text-sm">
      <thead>
        <tr className="text-gray-500 text-xs border-b border-gray-800">
          <th className="text-left py-2 font-normal">Command</th>
          <th className="text-left py-2 font-normal">Status</th>
          <th className="text-left py-2 font-normal">Requested By</th>
          <th className="text-left py-2 font-normal">Decided By</th>
          <th className="text-left py-2 font-normal">Requested</th>
          <th className="text-left py-2 font-normal">Decided</th>
        </tr>
      </thead>
      <tbody>
        {approvals.map(a => (
          <tr key={a.approval_id} className="border-b border-gray-800/50 hover:bg-gray-800/20">
            <td className="py-2 font-mono text-gray-200">{a.command_type}</td>
            <td className="py-2">
              <span className={`text-[10px] px-1.5 py-0.5 rounded ${statusBadge[a.status]}`}>
                {a.status}
              </span>
            </td>
            <td className="py-2 text-gray-400">{a.requested_by_email}</td>
            <td className="py-2 text-gray-400">{a.decided_by_email || '-'}</td>
            <td className="py-2 text-gray-500"><RelativeTime iso={a.requested_at} /></td>
            <td className="py-2 text-gray-500">{a.decided_at ? <RelativeTime iso={a.decided_at} /> : '-'}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
