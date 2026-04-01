/**
 * PhaseBreakdown — per-mode stacked phase timing bars, data table, and comparison deltas.
 * Phase colors: DNS=#3b82f6, TCP=#8b5cf6, TLS=#f59e0b, TTFB=#ef4444, Transfer=#10b981
 */

import { useState, useCallback } from 'react';

export interface PhaseData {
  mode: string;
  dns_ms: number | null;
  tcp_ms: number | null;
  tls_ms: number | null;
  ttfb_ms: number | null;
  transfer_ms: number | null;
  total_ms: number;
}

export interface ComparisonData {
  otherLanguage: string;
  otherColor: string;
  otherModes: PhaseData[];
}

export interface PhaseBreakdownProps {
  language: string;
  color: string;
  modes: PhaseData[];
  comparison?: ComparisonData;
}

interface PhaseDefinition {
  key: keyof Pick<PhaseData, 'dns_ms' | 'tcp_ms' | 'tls_ms' | 'ttfb_ms' | 'transfer_ms'>;
  label: string;
  color: string;
  dimmed?: boolean;
}

const PHASES: PhaseDefinition[] = [
  { key: 'dns_ms',      label: 'DNS',      color: '#3b82f6', dimmed: true },
  { key: 'tcp_ms',      label: 'TCP',      color: '#8b5cf6', dimmed: true },
  { key: 'tls_ms',      label: 'TLS',      color: '#f59e0b' },
  { key: 'ttfb_ms',     label: 'TTFB',     color: '#ef4444' },
  { key: 'transfer_ms', label: 'Transfer', color: '#10b981' },
];

function fmtMs(v: number | null): string {
  if (v === null) return '—';
  return `${v.toFixed(1)}ms`;
}

function fmtMsShort(v: number | null): string {
  if (v === null) return '—';
  return v.toFixed(1);
}

// ── Tooltip ──────────────────────────────────────────────────────────────────

interface TooltipState {
  x: number;
  y: number;
  label: string;
  value: number;
  pct: number;
}

// ── Stacked bar for one mode ─────────────────────────────────────────────────

interface ModeBarProps {
  data: PhaseData;
  maxTotal: number;
  barWidthPx: number;
  onTooltip: (t: TooltipState | null) => void;
}

function ModeBar({ data, maxTotal, barWidthPx, onTooltip }: ModeBarProps) {
  const allNull =
    data.dns_ms === null &&
    data.tcp_ms === null &&
    data.tls_ms === null &&
    data.ttfb_ms === null &&
    data.transfer_ms === null;

  const scaledTotal = maxTotal > 0 ? (data.total_ms / maxTotal) * barWidthPx : 0;

  const handleSegmentEnter = useCallback(
    (e: React.MouseEvent, label: string, value: number) => {
      const pct = data.total_ms > 0 ? (value / data.total_ms) * 100 : 0;
      onTooltip({ x: e.clientX, y: e.clientY, label, value, pct });
    },
    [data.total_ms, onTooltip]
  );

  const handleLeave = useCallback(() => onTooltip(null), [onTooltip]);

  if (allNull) {
    return (
      <div
        className="relative h-[10px] rounded-sm"
        style={{ width: `${scaledTotal}px`, backgroundColor: '#444' }}
        onMouseLeave={handleLeave}
        onMouseMove={(e) =>
          handleSegmentEnter(e, 'Total', data.total_ms)
        }
        title={`${data.total_ms.toFixed(1)}ms`}
      />
    );
  }

  const segments: Array<{ phase: PhaseDefinition; value: number }> = [];
  for (const phase of PHASES) {
    const v = data[phase.key];
    if (v !== null) {
      segments.push({ phase, value: v });
    }
  }

  return (
    <div className="flex h-[10px] rounded-sm overflow-hidden" style={{ width: `${scaledTotal}px` }}>
      {segments.map(({ phase, value }) => {
        const segW = maxTotal > 0 ? (value / maxTotal) * barWidthPx : 0;
        const displayW = Math.max(segW, 1);
        return (
          <div
            key={phase.key}
            style={{ width: `${displayW}px`, backgroundColor: phase.color, flexShrink: 0 }}
            onMouseMove={(e) => handleSegmentEnter(e, phase.label, value)}
            onMouseLeave={handleLeave}
          />
        );
      })}
    </div>
  );
}

// ── Delta badge ───────────────────────────────────────────────────────────────

interface DeltaInfo {
  value: number | null; // percent, null means unavailable
  isDimmed?: boolean;
  isBold?: boolean;
}

function DeltaBadge({ delta }: { delta: DeltaInfo }) {
  if (delta.value === null) {
    return <span style={{ color: '#555' }}>—</span>;
  }
  const v = delta.value;
  let color = '#22c55e'; // green — faster
  if (v > 20) color = '#ef4444';
  else if (v > 0) color = '#f59e0b';

  const displayColor = delta.isDimmed ? '#555' : color;
  const prefix = v > 0 ? '+' : '';
  const text = `${prefix}${Math.round(v)}%`;

  return (
    <span
      style={{ color: displayColor, fontWeight: delta.isBold ? 700 : 400 }}
      className="font-mono text-xs"
    >
      {text}
    </span>
  );
}

// ── Compute deltas for a pair of PhaseData ────────────────────────────────────

function computeDeltas(
  a: PhaseData,
  b: PhaseData
): Record<string, DeltaInfo> {
  // For each column, compute ((b - a) / a) * 100, where a is the faster (lower total)
  const [faster, slower] =
    a.total_ms <= b.total_ms ? [a, b] : [b, a];

  const phaseDeltaKeys: Array<{
    key: keyof Pick<PhaseData, 'dns_ms' | 'tcp_ms' | 'tls_ms' | 'ttfb_ms' | 'transfer_ms'>;
    dimmed: boolean;
  }> = [
    { key: 'dns_ms', dimmed: true },
    { key: 'tcp_ms', dimmed: true },
    { key: 'tls_ms', dimmed: false },
    { key: 'ttfb_ms', dimmed: false },
    { key: 'transfer_ms', dimmed: false },
  ];

  const rawDeltas: Record<string, number | null> = {};
  for (const { key, dimmed: _dimmed } of phaseDeltaKeys) {
    const fv = faster[key];
    const sv = slower[key];
    if (fv === null || sv === null || fv === 0) {
      rawDeltas[key] = null;
    } else {
      rawDeltas[key] = ((sv - fv) / fv) * 100;
    }
  }

  // Total delta
  if (faster.total_ms === 0) {
    rawDeltas['total_ms'] = null;
  } else {
    rawDeltas['total_ms'] = ((slower.total_ms - faster.total_ms) / faster.total_ms) * 100;
  }

  // Find largest absolute delta (non-null, non-dimmed) to bold
  const nonDimmedKeys = ['tls_ms', 'ttfb_ms', 'transfer_ms', 'total_ms'];
  let maxAbsDelta = 0;
  let maxKey = '';
  for (const k of nonDimmedKeys) {
    const v = rawDeltas[k];
    if (v !== null && Math.abs(v) > maxAbsDelta) {
      maxAbsDelta = Math.abs(v);
      maxKey = k;
    }
  }

  const result: Record<string, DeltaInfo> = {};
  for (const { key, dimmed } of phaseDeltaKeys) {
    result[key] = {
      value: rawDeltas[key] ?? null,
      isDimmed: dimmed,
      isBold: key === maxKey,
    };
  }
  result['total_ms'] = {
    value: rawDeltas['total_ms'] ?? null,
    isDimmed: false,
    isBold: 'total_ms' === maxKey,
  };

  return result;
}

// ── Main component ────────────────────────────────────────────────────────────

export function PhaseBreakdown({ language: _language, color, modes, comparison }: PhaseBreakdownProps) {
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);

  if (modes.length === 0) {
    return (
      <div className="bg-[#0a0f1a] border border-[#1e293b] rounded p-3 text-gray-500 text-xs font-mono">
        No phase data available
      </div>
    );
  }

  // Determine which modes appear in both sets for comparison
  const compModeMap = new Map<string, PhaseData>();
  if (comparison) {
    for (const m of comparison.otherModes) {
      compModeMap.set(m.mode, m);
    }
  }

  const sharedModes = comparison
    ? modes.filter((m) => compModeMap.has(m.mode))
    : modes;

  const displayModes = comparison ? sharedModes : modes;

  // Max total across all displayed modes (for bar scaling)
  const allTotals = displayModes.map((m) => m.total_ms);
  if (comparison) {
    for (const m of displayModes) {
      const other = compModeMap.get(m.mode);
      if (other) allTotals.push(other.total_ms);
    }
  }
  const maxTotal = Math.max(...allTotals, 0);

  // Bar container width: fixed 320px
  const BAR_WIDTH = 320;

  return (
    <div className="bg-[#0a0f1a] border border-[#1e293b] rounded p-3 space-y-3 relative">
      {/* ── Stacked bars ── */}
      <div className="space-y-[6px]">
        {displayModes.map((modeData) => (
          <div key={modeData.mode} className="flex items-center gap-3">
            {/* Mode label */}
            <div
              className="text-gray-500 text-xs font-mono w-24 shrink-0 truncate text-right"
              title={modeData.mode}
            >
              {modeData.mode}
            </div>
            {/* Bar */}
            <ModeBar
              data={modeData}
              maxTotal={maxTotal}
              barWidthPx={BAR_WIDTH}
              onTooltip={setTooltip}
            />
            {/* Total label */}
            <div className="text-gray-400 text-xs font-mono shrink-0">
              {modeData.total_ms.toFixed(1)}ms
            </div>
          </div>
        ))}
      </div>

      {/* ── Legend ── */}
      <div className="flex flex-wrap gap-3 pt-1">
        {PHASES.map((p) => (
          <div key={p.key} className="flex items-center gap-1">
            <div
              className="w-2.5 h-2.5 rounded-sm shrink-0"
              style={{ backgroundColor: p.color }}
            />
            <span className="text-gray-500 text-xs font-mono">{p.label}</span>
          </div>
        ))}
      </div>

      {/* ── Data table ── */}
      <div className="overflow-x-auto">
        <table className="w-full text-xs font-mono border-collapse">
          <thead>
            <tr className="text-gray-500 border-b border-[#1e293b]">
              <th className="text-left py-1 pr-2 font-normal">Mode</th>
              <th className="text-right py-1 px-1 font-normal">DNS</th>
              <th className="text-right py-1 px-1 font-normal">TCP</th>
              <th className="text-right py-1 px-1 font-normal">TLS</th>
              <th className="text-right py-1 px-1 font-normal">TTFB</th>
              <th className="text-right py-1 px-1 font-normal">Transfer</th>
              <th className="text-right py-1 pl-1 font-normal">Total</th>
            </tr>
          </thead>
          <tbody>
            {displayModes.map((modeData) => {
              const hasComparison = comparison && compModeMap.has(modeData.mode);
              const other = hasComparison ? compModeMap.get(modeData.mode)! : null;
              const deltas = other ? computeDeltas(modeData, other) : null;

              return (
                <>
                  {/* Primary row */}
                  <tr
                    key={`${modeData.mode}-primary`}
                    className="border-b border-[#1e293b] hover:bg-[#0f172a]"
                  >
                    <td className="py-1 pr-2 text-gray-300">{modeData.mode}</td>
                    <td className="text-right py-1 px-1 text-gray-400">
                      {fmtMsShort(modeData.dns_ms)}
                    </td>
                    <td className="text-right py-1 px-1 text-gray-400">
                      {fmtMsShort(modeData.tcp_ms)}
                    </td>
                    <td className="text-right py-1 px-1 text-gray-400">
                      {fmtMsShort(modeData.tls_ms)}
                    </td>
                    <td className="text-right py-1 px-1 text-gray-400">
                      {fmtMsShort(modeData.ttfb_ms)}
                    </td>
                    <td className="text-right py-1 px-1 text-gray-400">
                      {fmtMsShort(modeData.transfer_ms)}
                    </td>
                    <td className="text-right py-1 pl-1 font-semibold" style={{ color }}>
                      {modeData.total_ms.toFixed(1)}
                    </td>
                  </tr>

                  {/* Comparison row */}
                  {other && (
                    <tr
                      key={`${modeData.mode}-other`}
                      className="border-b border-[#1e293b] hover:bg-[#0f172a]"
                    >
                      <td className="py-1 pr-2" style={{ color: comparison!.otherColor }}>
                        {comparison!.otherLanguage}
                      </td>
                      <td className="text-right py-1 px-1 text-gray-400">
                        {fmtMsShort(other.dns_ms)}
                      </td>
                      <td className="text-right py-1 px-1 text-gray-400">
                        {fmtMsShort(other.tcp_ms)}
                      </td>
                      <td className="text-right py-1 px-1 text-gray-400">
                        {fmtMsShort(other.tls_ms)}
                      </td>
                      <td className="text-right py-1 px-1 text-gray-400">
                        {fmtMsShort(other.ttfb_ms)}
                      </td>
                      <td className="text-right py-1 px-1 text-gray-400">
                        {fmtMsShort(other.transfer_ms)}
                      </td>
                      <td className="text-right py-1 pl-1 font-semibold" style={{ color: comparison!.otherColor }}>
                        {other.total_ms.toFixed(1)}
                      </td>
                    </tr>
                  )}

                  {/* Delta row */}
                  {deltas && (
                    <tr
                      key={`${modeData.mode}-delta`}
                      className="border-b border-[#1e293b] bg-[#080d14]"
                    >
                      <td className="py-0.5 pr-2 text-gray-600 text-[10px]">Δ</td>
                      <td className="text-right py-0.5 px-1">
                        <DeltaBadge delta={deltas['dns_ms']} />
                      </td>
                      <td className="text-right py-0.5 px-1">
                        <DeltaBadge delta={deltas['tcp_ms']} />
                      </td>
                      <td className="text-right py-0.5 px-1">
                        <DeltaBadge delta={deltas['tls_ms']} />
                      </td>
                      <td className="text-right py-0.5 px-1">
                        <DeltaBadge delta={deltas['ttfb_ms']} />
                      </td>
                      <td className="text-right py-0.5 px-1">
                        <DeltaBadge delta={deltas['transfer_ms']} />
                      </td>
                      <td className="text-right py-0.5 pl-1">
                        <DeltaBadge delta={deltas['total_ms']} />
                      </td>
                    </tr>
                  )}
                </>
              );
            })}
          </tbody>
        </table>
      </div>

      {/* ── Hover Tooltip ── */}
      {tooltip && (
        <div
          className="fixed z-50 pointer-events-none bg-[#0f172a] border border-[#334155] rounded px-2 py-1 text-xs font-mono text-gray-200 shadow-lg"
          style={{ left: tooltip.x + 12, top: tooltip.y - 28 }}
        >
          {tooltip.label}: {fmtMs(tooltip.value)} ({tooltip.pct.toFixed(0)}% of total)
        </div>
      )}
    </div>
  );
}

