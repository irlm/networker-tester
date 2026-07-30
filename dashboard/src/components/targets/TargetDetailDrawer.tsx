import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../api/client';
import type { Deployment, DeploymentCostEstimate } from '../../api/types';
import { StatusBadge } from '../common/StatusBadge';
import { DetailList } from '../common/DetailList';
import { TargetEndpointCard } from './TargetEndpointCard';
import { formatDuration } from '../../lib/format';

interface TargetDetailDrawerProps {
  projectId: string;
  /** The selected deployment row (list shape is complete for deployments). */
  deployment: Deployment | null;
  isOperator: boolean;
  onClose: () => void;
  /** Open the upgrade wizard prefilled from this deployment (owner: Infrastructure page). */
  onUpgrade: (d: Deployment) => void;
}

function formatDate(value: string | null): string {
  if (!value) return '—';
  try {
    return new Date(value).toLocaleString();
  } catch {
    return value;
  }
}

/**
 * Target slide-over — the same select-to-inspect interaction runners have,
 * showing the identity core (via the shared TargetEndpointCard) without
 * leaving the Infrastructure page. The full page remains the home of the
 * deploy log + raw config; this drawer links to it.
 */
export function TargetDetailDrawer({
  projectId,
  deployment: row,
  isOperator,
  onClose,
  onUpgrade,
}: TargetDetailDrawerProps) {
  // Keyed by deployment id so switching targets can't show a stale estimate
  // (no synchronous reset needed in the effect).
  const [costFor, setCostFor] = useState<{ id: string; ce: DeploymentCostEstimate } | null>(null);

  const depId = row?.deployment_id;
  useEffect(() => {
    if (!depId) return;
    let cancelled = false;
    api.getDeploymentCostEstimate(projectId, depId)
      .then((ce) => { if (!cancelled) setCostFor({ id: depId, ce }); })
      .catch(() => { /* cost is optional decoration */ });
    return () => { cancelled = true; };
  }, [projectId, depId]);
  const costEstimate = costFor && costFor.id === depId ? costFor.ce : null;

  // Escape closes the drawer.
  useEffect(() => {
    if (!row) return;
    const handler = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [row, onClose]);

  if (!row) return null;
  const endpoints = row.config?.endpoints ?? [];
  const firstOs = endpoints[0]?.os ?? endpoints[0]?.azure?.os ?? endpoints[0]?.aws?.os ?? endpoints[0]?.gcp?.os;
  const canUpgrade = isOperator && row.status === 'completed'
    && (row.endpoint_ips?.length ?? 0) > 0 && firstOs !== 'windows';

  return (
    <div className="fixed inset-0 z-50 flex justify-end" data-testid="target-detail-drawer">
      <div
        className="absolute inset-0 bg-black/40 slide-over-backdrop"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="target-detail-title"
        className="relative w-full md:w-[560px] md:max-w-[95vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6 space-y-6">
          <div className="flex items-center justify-between">
            <div className="min-w-0">
              <h3 id="target-detail-title" className="text-lg font-bold text-gray-100 truncate">
                {row.name}
              </h3>
              <p className="text-xs text-gray-400 font-mono">
                {row.provider_summary ?? endpoints[0]?.provider ?? 'target'} · {row.deployment_id.slice(0, 8)}
              </p>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="text-gray-400 hover:text-gray-300 text-sm"
              aria-label="Close"
            >
              &#x2715;
            </button>
          </div>

          {/* ── Status ─────────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Status</h4>
            <div className="flex items-center gap-2 mb-2">
              <StatusBadge status={row.status} label={row.status} />
            </div>
            <DetailList
              rows={[
                { label: 'Deployed', value: row.started_at ? formatDate(row.started_at) : formatDate(row.created_at) },
                ...(row.started_at
                  ? [{ label: 'Duration', value: formatDuration(row.started_at, row.finished_at) }]
                  : []),
                { label: 'Created by', value: row.created_by },
              ]}
            />
          </section>

          {/* ── Infrastructure — same identity core as the runner drawer ── */}
          {endpoints.map((ep, i) => (
            <section key={i}>
              <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">
                {endpoints.length > 1 ? `Endpoint ${i + 1}` : 'Identity'}
              </h4>
              <TargetEndpointCard
                bare
                ep={ep}
                index={i}
                ip={row.endpoint_ips?.[i]}
                cost={costEstimate?.endpoints[i]}
              />
            </section>
          ))}

          {row.error_message && (
            <section>
              <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Error</h4>
              <p className="text-xs text-red-400">{row.error_message}</p>
            </section>
          )}

          {/* ── Actions ────────────────────────────────────────────────── */}
          <section className="flex flex-wrap gap-2">
            <Link
              to={`/projects/${projectId}/deploy/${row.deployment_id}`}
              className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-300 hover:border-gray-600"
            >
              Open full page (log & config) →
            </Link>
            <Link
              to={`/projects/${projectId}/network/${row.deployment_id}`}
              className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-300 hover:border-gray-600"
            >
              ↗ Runs
            </Link>
            {canUpgrade && (
              <button
                type="button"
                onClick={() => onUpgrade(row)}
                className="px-3 py-1 text-xs rounded border border-cyan-700 text-cyan-300 hover:bg-cyan-900/30"
              >
                + Upgrade test support
              </button>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
