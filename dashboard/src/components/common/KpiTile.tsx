import type { ReactNode } from 'react';

/**
 * The canonical KPI tile (kpi-strip/v2 pick): a 2px left border encodes
 * health pre-attentively, healthy tiles stay quiet, and the `sub` line
 * carries the denominator/reason. Replaces three private implementations
 * (PerfLogPage StatCard, TlsProfileDetailPage MetricCard, DashboardPage's
 * inline KPI divs) that had drifted on typography and color logic.
 *
 * Health is a STATUS, so it uses the pill palette (green/yellow/red +
 * cyan for in-flight) — not the cyan magnitude ramp.
 */
export type KpiHealth = 'ok' | 'warn' | 'err' | 'info' | 'muted';

const BORDER: Record<KpiHealth, string> = {
  ok: 'border-l-green-400',
  warn: 'border-l-yellow-400',
  err: 'border-l-red-400',
  info: 'border-l-cyan-400',
  muted: 'border-l-gray-700',
};

const VALUE: Record<KpiHealth, string> = {
  ok: 'text-green-400',
  warn: 'text-yellow-400',
  err: 'text-red-400',
  info: 'text-cyan-400',
  muted: 'text-gray-300',
};

interface KpiTileProps {
  label: string;
  value: ReactNode;
  health?: KpiHealth;
  /** Denominator, delta, or the reason when health !== 'ok'. */
  sub?: ReactNode;
  /** Native tooltip on the tile. */
  title?: string;
  /**
   * Override the value color with a magnitude-ramp class (text-s2/s3/s4/
   * breach) when the number IS a magnitude rather than a status. The left
   * border still follows `health`.
   */
  valueClass?: string;
}

export function KpiTile({ label, value, health = 'muted', sub, title, valueClass }: KpiTileProps) {
  return (
    <div
      title={title}
      className={`border border-gray-800 border-l-2 ${BORDER[health]} bg-[var(--bg-surface)] rounded px-4 py-3`}
    >
      <p className="text-[10px] text-gray-400 tracking-wider uppercase mb-1">{label}</p>
      <p className={`text-2xl font-bold tabular-nums ${valueClass ?? VALUE[health]}`}>{value}</p>
      {sub && <p className="text-xs text-gray-500 mt-1">{sub}</p>}
    </div>
  );
}
