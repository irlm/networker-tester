import { useState } from 'react';
import type { TesterRow } from '../api/testers';
import type { TesterQueueState } from '../hooks/useTesterSubscription';
import { StatusBadge } from './common/StatusBadge';

interface TesterRegionGroupProps {
  cloud: string;
  region: string;
  testers: TesterRow[];
  queues: Record<string, TesterQueueState>;
  onSelect: (tester: TesterRow) => void;
  onAdd: (cloud: string, region: string) => void;
}

function stateBadgeStatus(
  power: TesterRow['power_state'],
  allocation: TesterRow['allocation'],
): string {
  if (power === 'error') return 'failed';
  if (power === 'running' && allocation === 'locked') return 'running';
  if (power === 'running') return 'online';
  if (power === 'starting' || power === 'provisioning') return 'deploying';
  if (power === 'stopping') return 'pending';
  if (power === 'stopped') return 'offline';
  if (power === 'upgrading') return 'deploying';
  return 'offline';
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
            {testers.length} tester{testers.length === 1 ? '' : 's'} ·{' '}
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
              <li
                key={t.tester_id}
                className="px-3 py-2 flex items-center gap-3 text-xs hover:bg-gray-900/40 cursor-pointer"
                onClick={() => onSelect(t)}
              >
                <div className="w-28">
                  <StatusBadge
                    status={stateBadgeStatus(t.power_state, t.allocation)}
                    label={t.power_state}
                  />
                </div>
                <div className="flex-1 min-w-0">
                  <div className="text-gray-200 font-mono truncate">{t.name}</div>
                  <div className="text-gray-500 truncate">
                    {t.vm_size} · v{t.installer_version ?? '—'}
                  </div>
                </div>
                <div className="w-24 text-right text-gray-400 font-mono">
                  {q?.running ? 'running' : q?.queued.length ? `${q.queued.length} queued` : 'idle'}
                </div>
              </li>
            );
          })}
          {testers.length === 0 && (
            <li className="px-3 py-3 text-xs text-gray-500">No testers in this region.</li>
          )}
        </ul>
      )}
    </div>
  );
}
