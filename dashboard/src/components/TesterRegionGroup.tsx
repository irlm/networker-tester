import { useState } from 'react';
import type { TesterRow } from '../api/testers';
import type { TesterQueueState } from '../hooks/useTesterSubscription';
import { StatusBadge } from './common/StatusBadge';
import { timeAgo } from '../lib/format';

/** True when the agent's api-key expiry is non-null and already past. */
function keyExpired(t: TesterRow): boolean {
  return Boolean(
    t.api_key_expires_at && new Date(t.api_key_expires_at).getTime() <= Date.now(),
  );
}

interface TesterRegionGroupProps {
  cloud: string;
  region: string;
  testers: TesterRow[];
  queues: Record<string, TesterQueueState>;
  onSelect: (tester: TesterRow) => void;
  onAdd: (cloud: string, region: string) => void;
}

// One merged runner state (VM power × agent allocation) shown in ONE place —
// the badge. The old layout painted a green "running" badge (VM power) next
// to right-aligned "idle" (agent state) with no affordance distinguishing the
// two vocabularies, and "stopped" twice (audit F7).
function runnerState(
  t: TesterRow,
  q: TesterQueueState | undefined,
): { badge: string; label: string } {
  if (t.power_state === 'error') return { badge: 'failed', label: 'error' };
  if (t.power_state === 'stopped') return { badge: 'offline', label: 'stopped' };
  if (t.power_state === 'stopping') return { badge: 'pending', label: 'stopping' };
  if (t.power_state === 'starting' || t.power_state === 'provisioning' || t.power_state === 'upgrading') {
    return { badge: 'deploying', label: t.power_state };
  }
  if (q?.running || t.allocation === 'locked' || t.allocation === 'upgrading') {
    return { badge: 'busy', label: 'busy' };
  }
  return { badge: 'online', label: 'idle' };
}

// Right column carries queue detail only — never a second status vocabulary.
function queueDetail(q: TesterQueueState | undefined): string | null {
  if (q?.running) return 'running job';
  if (q?.queued.length) return `${q.queued.length} queued`;
  return null;
}

export function TesterRegionGroup({
  cloud,
  region,
  testers,
  queues,
  onSelect,
  onAdd,
}: TesterRegionGroupProps) {
  const [expanded, setExpanded] = useState(true);

  const runningCount = testers.filter(
    (t) => t.power_state === 'running' && t.allocation !== 'idle',
  ).length;
  const inQueueCount = testers.reduce((sum, t) => {
    const q = queues[t.tester_id];
    return sum + (q?.queued?.length ?? 0);
  }, 0);

  return (
    <div className="border border-gray-800 rounded mb-3 bg-[var(--bg-surface)]">
      <div className="flex items-center justify-between px-3 py-2 border-b border-gray-800">
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="flex items-center gap-2 text-sm text-gray-200 hover:text-cyan-400"
        >
          <span className="text-xs">{expanded ? '▾' : '▸'}</span>
          <span className="font-mono">
            {cloud} / {region}
          </span>
          <span className="text-xs text-gray-500">
            {testers.length} runner{testers.length === 1 ? '' : 's'} ·{' '}
            {runningCount} running · {inQueueCount} in queue
          </span>
        </button>
        <button
          type="button"
          onClick={() => onAdd(cloud, region)}
          className="px-2 py-0.5 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400"
        >
          + add to {region}
        </button>
      </div>
      {expanded && (
        <ul className="divide-y divide-gray-900">
          {testers.map((t) => {
            const q = queues[t.tester_id];
            return (
              <li key={t.tester_id}>
                {/* Full-row button so the drawer opens via keyboard too. */}
                <button
                  type="button"
                  onClick={() => onSelect(t)}
                  className="w-full text-left px-3 py-2 flex items-center gap-3 text-xs hover:bg-gray-900/40 focus-visible:outline-none focus-visible:bg-gray-900/40"
                >
                  {(() => {
                    const s = runnerState(t, q);
                    const detail = queueDetail(q);
                    return (
                      <>
                        <div className="w-28">
                          <StatusBadge status={s.badge} label={s.label} />
                        </div>
                        <div className="flex-1 min-w-0">
                          <div className="flex items-center gap-2">
                            <span className="text-gray-200 font-mono truncate">{t.name}</span>
                            {keyExpired(t) && (
                              <StatusBadge status="failed" label="key expired" />
                            )}
                          </div>
                          <div className="text-gray-500 truncate">
                            {t.vm_size} · v{t.installer_version ?? '?'}
                          </div>
                        </div>
                        <div className="w-32 text-right font-mono text-gray-500">
                          <div>{detail ?? ''}</div>
                          <div
                            className="text-gray-600"
                            title="last time the runner's agent key authenticated"
                          >
                            {t.api_key_last_used_at
                              ? `seen ${timeAgo(t.api_key_last_used_at)}`
                              : 'never seen'}
                          </div>
                        </div>
                      </>
                    );
                  })()}
                </button>
              </li>
            );
          })}
          {testers.length === 0 && (
            <li className="px-3 py-3 text-xs text-gray-500">No runners in this region.</li>
          )}
        </ul>
      )}
    </div>
  );
}
