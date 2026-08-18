import { useState } from 'react';
import { api } from '../api/client';
import type { PendingProject } from '../api/client';
import { Button } from './common/Button';
import { Modal } from './common/Modal';

interface PendingProjectsModalProps {
  projects: PendingProject[];
  onComplete: () => void;
}

const ROLE_COLORS: Record<string, string> = {
  admin: 'text-purple-400 border-purple-700 bg-purple-950/40',
  operator: 'text-cyan-400 border-cyan-700 bg-cyan-950/40',
  viewer: 'text-gray-400 border-gray-700 bg-gray-900/40',
};

function roleBadge(role: string) {
  const cls = ROLE_COLORS[role] ?? ROLE_COLORS.viewer;
  return (
    <span className={`inline-block text-xs uppercase tracking-wider border rounded px-1.5 py-0.5 ${cls}`}>
      {role}
    </span>
  );
}

export function PendingProjectsModal({ projects: initialProjects, onComplete }: PendingProjectsModalProps) {
  const [items, setItems] = useState<PendingProject[]>(initialProjects);
  const [busy, setBusy] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<Record<string, string>>({});

  const remove = (projectId: string) =>
    setItems(prev => prev.filter(p => p.project_id !== projectId));

  const handleAccept = async (p: PendingProject) => {
    setBusy(p.project_id);
    try {
      await api.acceptProject(p.project_id);
      setFeedback(prev => ({ ...prev, [p.project_id]: 'accepted' }));
      setTimeout(() => remove(p.project_id), 600);
    } catch {
      setFeedback(prev => ({ ...prev, [p.project_id]: 'error' }));
    } finally {
      setBusy(null);
    }
  };

  const handleDeny = async (p: PendingProject) => {
    setBusy(p.project_id);
    try {
      await api.denyProject(p.project_id);
      remove(p.project_id);
    } catch {
      setFeedback(prev => ({ ...prev, [p.project_id]: 'error' }));
    } finally {
      setBusy(null);
    }
  };

  const handleIgnore = (projectId: string) => {
    remove(projectId);
  };

  return (
    <Modal
      onClose={onComplete}
      labelledBy="pending-projects-title"
      maxWidth="max-w-md"
      closeDisabled={busy !== null}
      panelClassName="!p-0 overflow-hidden"
    >
        {/* Header */}
        <div className="px-5 py-4 border-b border-[var(--border-default)] flex items-center justify-between">
          <div>
            <h2 id="pending-projects-title" className="text-sm font-semibold text-gray-100 tracking-tight">
              Pending Project Invitations
            </h2>
            <p className="text-xs text-gray-400 mt-0.5">
              {items.length === 0
                ? 'All invitations handled.'
                : `${items.length} invitation${items.length !== 1 ? 's' : ''} awaiting your response`}
            </p>
          </div>
        </div>

        {/* Invitation list */}
        <div className="px-5 py-3 max-h-72 overflow-y-auto">
          {items.length === 0 ? (
            <p className="text-xs text-gray-400 py-4 text-center">No more pending invitations.</p>
          ) : (
            <ul className="space-y-2">
              {items.map(p => {
                const fb = feedback[p.project_id];
                const isBusy = busy === p.project_id;
                return (
                  <li
                    key={p.project_id}
                    className={`rounded border border-[var(--border-default)] bg-[var(--bg-raised)] px-4 py-3 transition-opacity ${fb === 'accepted' ? 'opacity-40' : ''}`}
                  >
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="flex items-center gap-2 mb-1">
                          <span className="text-sm font-semibold text-gray-100 truncate">{p.project_name}</span>
                          {roleBadge(p.role)}
                        </div>
                        {p.invited_by_email && (
                          <p className="text-xs text-gray-400 truncate">
                            invited by {p.invited_by_email}
                          </p>
                        )}
                        {fb === 'error' && (
                          <p className="text-xs text-red-400 mt-1">Action failed — try again</p>
                        )}
                        {fb === 'accepted' && (
                          <p className="text-xs text-green-400 mt-1">Accepted</p>
                        )}
                      </div>
                    </div>
                    <div className="flex items-center gap-2 mt-3">
                      <Button
                        variant="primary"
                        size="xs"
                        disabled={isBusy}
                        onClick={() => handleAccept(p)}
                        loading={isBusy}
                        loadingLabel="…"
                        className="flex-1"
                      >
                        Accept
                      </Button>
                      <Button
                        variant="danger"
                        size="xs"
                        disabled={isBusy}
                        onClick={() => handleDeny(p)}
                        className="flex-1"
                      >
                        Deny
                      </Button>
                      <Button
                        size="xs"
                        disabled={isBusy}
                        onClick={() => handleIgnore(p.project_id)}
                        className="flex-1"
                      >
                        Ignore
                      </Button>
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        {/* Footer */}
        <div className="px-5 py-4 border-t border-[var(--border-default)] flex justify-end">
          <Button variant="primary" size="xs" onClick={onComplete}>
            Continue
          </Button>
        </div>
    </Modal>
  );
}
