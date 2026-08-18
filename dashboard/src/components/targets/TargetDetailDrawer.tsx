import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../api/client';
import type { Deployment, DeploymentCostEstimate } from '../../api/types';
import { StatusBadge } from '../common/StatusBadge';
import { DetailList } from '../common/DetailList';
import { TargetEndpointCard } from './TargetEndpointCard';
import { formatDuration } from '../../lib/format';
import { Modal } from '../common/Modal';
import { Button } from '../common/Button';
import { buttonClassName } from '../common/button-styles';

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

  if (!row) return null;
  const endpoints = row.config?.endpoints ?? [];
  const firstOs = endpoints[0]?.os ?? endpoints[0]?.azure?.os ?? endpoints[0]?.aws?.os ?? endpoints[0]?.gcp?.os;
  const canUpgrade = isOperator && row.status === 'completed'
    && (row.endpoint_ips?.length ?? 0) > 0 && firstOs !== 'windows';

  return (
    <Modal
      onClose={onClose}
      labelledBy="target-detail-title"
      variant="slide-over"
      panelClassName="md:w-[560px] md:max-w-[95vw]"
      testId="target-detail-drawer"
    >
        <div className="p-4 md:p-6 space-y-6">
          <div className="flex items-center justify-between">
            <div className="min-w-0">
              <h3 id="target-detail-title" className="text-lg font-bold text-gray-100 truncate">
                {row.name}
              </h3>
              <p className="text-xs text-gray-400">
                {row.provider_summary ?? endpoints[0]?.provider ?? 'target'} · {row.deployment_id.slice(0, 8)}
              </p>
            </div>
            <Button
              onClick={onClose}
              variant="ghost"
              size="xs"
              aria-label="Close"
            >
              &#x2715;
            </Button>
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
              className={buttonClassName({ size: 'xs' })}
            >
              Open full page (log & config) →
            </Link>
            <Link
              to={`/projects/${projectId}/network/${row.deployment_id}`}
              className={buttonClassName({ size: 'xs' })}
            >
              ↗ Runs
            </Link>
            {canUpgrade && (
              <Button
                onClick={() => onUpgrade(row)}
                variant="primary"
                size="xs"
              >
                + Upgrade test support
              </Button>
            )}
          </section>
        </div>
    </Modal>
  );
}
