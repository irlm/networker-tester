import type { ReactNode } from 'react';

export interface DetailRow {
  label: string;
  value: ReactNode;
  /** Render the value in the accent color (e.g. the actionable cost figure). */
  accent?: boolean;
  /** Optional hover tooltip (e.g. kernel string behind the OS row). */
  title?: string;
}

/**
 * Shared label/value grid for resource detail views (runner drawer, target
 * detail). One component so runners and targets present identity, cost, and
 * usage facts identically — same order, same typography.
 * Nullish/empty values render as an em dash rather than being dropped, so the
 * field list is stable across resources and gaps are visible.
 */
export function DetailList({ rows }: { rows: DetailRow[] }) {
  return (
    <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
      {rows.map((r) => (
        <div key={r.label} className="contents">
          <dt className="text-gray-400">{r.label}</dt>
          <dd
            className={`${r.accent ? 'text-cyan-400' : 'text-gray-300'} font-mono`}
            title={r.title}
          >
            {r.value ?? '—'}
          </dd>
        </div>
      ))}
    </dl>
  );
}
