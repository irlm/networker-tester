import { useState, useCallback } from 'react';
import { api, errorMessage } from '../api/client';
import type { ShareLink } from '../api/types';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { SettingsTabs } from '../components/common/SettingsTabs';
import { DataTable } from '../components/common/DataTable';

function statusBadge(link: ShareLink) {
  if (link.revoked) {
    return <span className="text-xs px-2 py-0.5 rounded bg-red-500/15 text-red-400 border border-red-500/30">revoked</span>;
  }
  if (new Date(link.expires_at) < new Date()) {
    return <span className="text-xs px-2 py-0.5 rounded bg-gray-500/15 text-gray-400 border border-gray-500/30">expired</span>;
  }
  return <span className="text-xs px-2 py-0.5 rounded bg-green-500/15 text-green-400 border border-green-500/30">active</span>;
}

export function ShareLinksPage() {
  const { projectId, isProjectAdmin } = useProject();
  const [links, setLinks] = useState<ShareLink[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);

  usePageTitle('Settings');

  const fetchLinks = useCallback(() => {
    if (!projectId) return;
    api.getShareLinks(projectId)
      .then(data => { setLinks(data); setError(null); setLoading(false); })
      .catch(e => { setError(errorMessage(e)); setLoading(false); });
  }, [projectId]);

  usePolling(fetchLinks, 15000, !!projectId);

  const handleRevoke = async (linkId: string) => {
    if (!projectId) return;
    setActionInProgress(linkId);
    try {
      await api.revokeShareLink(projectId, linkId);
      fetchLinks();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setActionInProgress(null);
    }
  };

  const handleExtend = async (linkId: string) => {
    if (!projectId) return;
    setActionInProgress(linkId);
    try {
      await api.extendShareLink(projectId, linkId, 30);
      fetchLinks();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setActionInProgress(null);
    }
  };

  const handleDelete = async (linkId: string) => {
    if (!projectId || !confirm('Delete this share link permanently?')) return;
    setActionInProgress(linkId);
    try {
      await api.deleteShareLink(projectId, linkId);
      fetchLinks();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setActionInProgress(null);
    }
  };

  return (
    <div className="p-4 md:p-6">
      <Breadcrumb items={[{ label: 'Share Links' }]} />

      <div className="mb-6">
        <h2 className="text-xl font-bold text-gray-100 mb-1">Settings</h2>
        <p className="text-sm text-gray-400">Manage external share links for runs and tests.</p>
      </div>
      <SettingsTabs />

      {error && (
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-3 mb-4 text-sm text-red-400">
          {error}
        </div>
      )}

      {loading ? (
        <div className="text-gray-400 motion-safe:animate-pulse">Loading share links...</div>
      ) : links.length === 0 ? (
        <div className="text-center py-16 text-faint">
          <p className="text-lg mb-2">No share links</p>
          <p className="text-sm">Share links can be created from run and test detail pages.</p>
        </div>
      ) : (
        <DataTable
          columns={[
            {
              key: 'label',
              label: 'Label',
              cellClass: 'text-gray-200',
              render: (link) => link.label || <span className="text-faint italic">no label</span>,
            },
            { key: 'type', label: 'Type', cellClass: 'text-gray-400', render: (link) => link.resource_type },
            { key: 'status', label: 'Status', render: (link) => statusBadge(link) },
            { key: 'views', label: 'Views', align: 'right', cellClass: 'text-gray-400', render: (link) => link.access_count },
            {
              key: 'expires',
              label: 'Expires',
              cellClass: 'text-gray-400',
              render: (link) => new Date(link.expires_at).toLocaleDateString(),
            },
            { key: 'created_by', label: 'Created By', cellClass: 'text-gray-400', render: (link) => link.created_by_email },
            {
              key: 'actions',
              label: 'Actions',
              align: 'right',
              render: (link) => {
                const isActive = !link.revoked && new Date(link.expires_at) > new Date();
                const busy = actionInProgress === link.link_id;
                return (
                  isProjectAdmin && (
                    <div className="flex items-center justify-end gap-2">
                      {isActive && (
                        <>
                          <button
                            onClick={() => handleRevoke(link.link_id)}
                            disabled={busy}
                            className="text-yellow-500 hover:text-yellow-400 disabled:opacity-50"
                          >
                            Revoke
                          </button>
                          <button
                            onClick={() => handleExtend(link.link_id)}
                            disabled={busy}
                            className="text-cyan-500 hover:text-cyan-400 disabled:opacity-50"
                          >
                            +30d
                          </button>
                        </>
                      )}
                      <button
                        onClick={() => handleDelete(link.link_id)}
                        disabled={busy}
                        className="text-red-500 hover:text-red-400 disabled:opacity-50"
                      >
                        Delete
                      </button>
                    </div>
                  )
                );
              },
            },
          ]}
          rows={links}
          rowKey={(link) => link.link_id}
        />
      )}
    </div>
  );
}
