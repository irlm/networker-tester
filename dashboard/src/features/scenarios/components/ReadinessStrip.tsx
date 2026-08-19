import { Link } from 'react-router';
import { Button } from '../../../components/common/Button';
import type { ReadinessSummaryItem } from '../model';

const TONE_CLASS: Record<ReadinessSummaryItem['tone'], string> = {
  ready: 'text-green-400',
  attention: 'text-yellow-400',
  blocked: 'text-red-400',
  unverified: 'text-gray-400',
};

const DOT_CLASS: Record<ReadinessSummaryItem['tone'], string> = {
  ready: 'bg-green-400',
  attention: 'bg-yellow-400',
  blocked: 'bg-red-400',
  unverified: 'bg-gray-500',
};

interface ReadinessStripProps {
  items: ReadinessSummaryItem[];
  projectId: string;
  expanded: boolean;
  onToggle: () => void;
  onRetry: () => void;
  refreshing: boolean;
  checkedAt?: number;
}

export function ReadinessStrip({
  items,
  projectId,
  expanded,
  onToggle,
  onRetry,
  refreshing,
  checkedAt,
}: ReadinessStripProps) {
  const hasUnavailableData = items.some((item) => item.tone === 'unverified');

  return (
    <section className="border-y border-[var(--border-default)]" aria-labelledby="readiness-heading">
      <div className="flex flex-col gap-3 py-3 lg:flex-row lg:items-center">
        <div className="flex items-center gap-3 lg:w-44 lg:flex-shrink-0">
          <h3 id="readiness-heading" className="section-label">Project readiness</h3>
          {refreshing && <span className="text-xs text-cyan-400 motion-safe:animate-pulse">checking…</span>}
        </div>
        <div className="grid flex-1 gap-x-6 gap-y-2 sm:grid-cols-3">
          {items.map((item) => (
            <div key={item.id} className="flex min-w-0 items-center gap-2 text-xs">
              <span className={`h-1.5 w-1.5 flex-shrink-0 rounded-full ${DOT_CLASS[item.tone]}`} aria-hidden="true" />
              <span className="text-gray-400">{item.label}</span>
              <span className={`truncate font-semibold ${TONE_CLASS[item.tone]}`}>{item.value}</span>
            </div>
          ))}
        </div>
        <div className="flex items-center gap-2 lg:flex-shrink-0">
          {hasUnavailableData && (
            <Button size="xs" variant="ghost" onClick={onRetry} loading={refreshing} loadingLabel="Checking…">
              Retry
            </Button>
          )}
          <Button
            size="xs"
            variant="ghost"
            onClick={onToggle}
            aria-expanded={expanded}
            aria-controls="readiness-details"
          >
            {expanded ? 'Hide status' : 'View status'}
          </Button>
        </div>
      </div>

      {expanded && (
        <div id="readiness-details" className="grid gap-px border-t border-[var(--border-default)] bg-[var(--border-default)] md:grid-cols-3">
          {items.map((item) => (
            <div key={item.id} className="bg-[var(--bg-base)] px-3 py-3 text-xs">
              <div className="mb-1 flex items-center justify-between gap-3">
                <span className={`font-semibold ${TONE_CLASS[item.tone]}`}>{item.label}: {item.value}</span>
                <Link className="text-cyan-400 hover:text-cyan-300" to={item.repairPath(projectId)}>
                  {item.repairLabel} →
                </Link>
              </div>
              <p className="text-gray-400 leading-relaxed">{item.detail}</p>
            </div>
          ))}
          <p className="bg-[var(--bg-base)] px-3 pb-3 text-xs text-faint md:col-span-3">
            Checked through project APIs{checkedAt ? ` at ${new Date(checkedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}` : ''}. Refreshes every 15 seconds.
          </p>
        </div>
      )}
    </section>
  );
}
