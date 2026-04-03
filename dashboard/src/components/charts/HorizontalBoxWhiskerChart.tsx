/**
 * Horizontal box-and-whisker chart — each row is a language/group, X-axis is the metric.
 * Sorted by p50 ascending (fastest first). SVG-based, no Recharts dependency.
 */

import { useMemo, useState, useRef, useCallback } from 'react';

export interface HBoxGroup {
  label: string;
  sublabel?: string;
  color: string;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  mean: number;
}

export interface HorizontalBoxWhiskerProps {
  groups: HBoxGroup[];
  unit: string;
  title?: string;
  onClickGroup?: (label: string) => void;
  expandedGroups?: Set<string>;
}

function fmt(v: number, unit: string): string {
  if (unit !== 'ms') return `${v.toFixed(1)}${unit}`;
  if (v >= 1000) return `${(v / 1000).toFixed(1)}s`;
  if (v >= 100) return `${Math.round(v)}ms`;
  if (v >= 10) return `${v.toFixed(1)}ms`;
  if (v >= 1) return `${v.toFixed(2)}ms`;
  return `${(v * 1000).toFixed(0)}µs`;
}

/** Generate nice tick values for the X axis */
function niceGridTicks(maxVal: number): number[] {
  if (maxVal <= 0) return [0];

  const ticks: number[] = [];

  if (maxVal > 100) {
    // Power-of-10 intervals
    const magnitude = Math.pow(10, Math.floor(Math.log10(maxVal)));
    const step = magnitude >= maxVal / 5 ? magnitude / 5 : magnitude;
    for (let t = 0; t <= maxVal * 1.05; t += step) {
      ticks.push(Math.round(t));
    }
  } else {
    // 1/2/5 nice intervals
    const rawStep = maxVal / 5;
    let step: number;
    if (rawStep <= 1) step = 1;
    else if (rawStep <= 2) step = 2;
    else if (rawStep <= 5) step = 5;
    else if (rawStep <= 10) step = 10;
    else if (rawStep <= 20) step = 20;
    else step = 50;

    for (let t = 0; t <= maxVal * 1.05; t += step) {
      ticks.push(parseFloat(t.toFixed(4)));
    }
  }

  // Deduplicate and cap at domain
  return [...new Set(ticks)].filter(t => t <= maxVal * 1.15);
}

interface TooltipState {
  visible: boolean;
  x: number;
  y: number;
  group: HBoxGroup | null;
}

export function HorizontalBoxWhiskerChart({
  groups,
  unit,
  title,
  onClickGroup,
  expandedGroups,
}: HorizontalBoxWhiskerProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [tooltip, setTooltip] = useState<TooltipState>({ visible: false, x: 0, y: 0, group: null });

  // Sort by p50 ascending (fastest first)
  const rows = useMemo(
    () => [...groups].filter(g => g.p95 >= 0).sort((a, b) => a.p50 - b.p50),
    [groups],
  );

  const handleMouseMove = useCallback(
    (e: React.MouseEvent<SVGElement>, group: HBoxGroup) => {
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      setTooltip({
        visible: true,
        x: e.clientX - rect.left + 12,
        y: e.clientY - rect.top - 8,
        group,
      });
    },
    [],
  );

  const handleMouseLeave = useCallback(() => {
    setTooltip(prev => ({ ...prev, visible: false }));
  }, []);

  if (rows.length === 0) return null;

  // Layout constants — wider viewBox = smaller rendered fonts
  const longestLabel = Math.max(...rows.map(r => r.label.length), 4);
  const LBL_W = Math.max(100, longestLabel * 7 + 20); // ~7px per char at fontSize 9
  const MEAN_W = 65;     // right annotation area for mean text
  const ROW_H = 24;      // tighter rows
  const BOX_H = 12;
  const PAD_TOP = title ? 28 : 6;
  const PAD_BOT = 20;    // space for x-axis labels
  const PAD_LEFT = 4;    // small left margin inside chart area

  // Domain: 0 to max(p95) * 1.1
  const maxP95 = Math.max(...rows.map(r => r.p95));
  const domainMax = maxP95 > 0 ? maxP95 * 1.1 : 1;

  // Ticks
  const ticks = niceGridTicks(domainMax);

  // We'll compute the chart area width dynamically via viewBox but use a stable
  // internal width. The SVG uses width="100%" with a viewBox — responsive.
  const CHART_W = LBL_W + MEAN_W + PAD_LEFT + 500; // wider viewBox = fonts render smaller on screen
  const CHART_AREA = CHART_W - LBL_W - MEAN_W - PAD_LEFT;
  const totalH = PAD_TOP + rows.length * ROW_H + PAD_BOT;

  const scaleX = (v: number) =>
    LBL_W + PAD_LEFT + Math.max(0, (v / domainMax) * CHART_AREA);

  return (
    <div ref={containerRef} className="relative w-full">
      {title && (
        <div
          className="mb-2 text-xs uppercase tracking-wider text-gray-400"
          style={{ fontFamily: 'var(--font-mono, ui-monospace, monospace)' }}
        >
          {title}
        </div>
      )}

      <svg
        width="100%"
        viewBox={`0 0 ${CHART_W} ${totalH}`}
        preserveAspectRatio="xMinYMin meet"
        style={{ fontFamily: 'var(--font-mono, ui-monospace, monospace)', fontSize: 9, display: 'block' }}
      >
        {/* Grid lines */}
        {ticks.map(t => {
          const x = scaleX(t);
          return (
            <g key={t}>
              <line
                x1={x}
                y1={PAD_TOP}
                x2={x}
                y2={PAD_TOP + rows.length * ROW_H}
                stroke="#1e293b"
                strokeWidth={1}
              />
              <text
                x={x}
                y={PAD_TOP + rows.length * ROW_H + 13}
                textAnchor="middle"
                fill="#6b7280"
                fontSize={8}
              >
                {fmt(t, unit)}
              </text>
            </g>
          );
        })}

        {/* Rows */}
        {rows.map((row, i) => {
          const y0 = PAD_TOP + i * ROW_H;
          const cy = y0 + ROW_H / 2;
          const boxTop = cy - BOX_H / 2;

          const p5x = scaleX(row.p5);
          const q1x = scaleX(row.p25);
          const medx = scaleX(row.p50);
          const q3x = scaleX(row.p75);
          const p95x = scaleX(row.p95);

          // Min 1px segment width
          const boxW = Math.max(q3x - q1x, 1);
          const medW = 2;

          const isExpanded = expandedGroups?.has(row.label) ?? false;

          return (
            <g
              key={row.label}
              style={{ cursor: onClickGroup ? 'pointer' : 'default' }}
              onClick={() => onClickGroup?.(row.label)}
              onMouseMove={e => handleMouseMove(e, row)}
              onMouseLeave={handleMouseLeave}
            >
              {/* Expanded left border accent */}
              {isExpanded && (
                <rect
                  x={LBL_W - 3}
                  y={boxTop}
                  width={2}
                  height={BOX_H}
                  fill={row.color}
                />
              )}

              {/* Label — right-aligned */}
              <text
                x={LBL_W - 6}
                y={cy - (row.sublabel ? 3 : 0)}
                textAnchor="end"
                dominantBaseline="middle"
                fill="#6b7280"
                fontSize={9}
              >
                {row.label}
              </text>
              {row.sublabel && (
                <text
                  x={LBL_W - 6}
                  y={cy + 7}
                  textAnchor="end"
                  dominantBaseline="middle"
                  fill="#4b5563"
                  fontSize={7}
                >
                  {row.sublabel}
                </text>
              )}

              {/* Whisker line: p5 to p95 */}
              <line
                x1={p5x}
                y1={cy}
                x2={p95x}
                y2={cy}
                stroke="#444"
                strokeWidth={2}
              />

              {/* p5 end cap */}
              <line x1={p5x} y1={cy - 4} x2={p5x} y2={cy + 4} stroke="#444" strokeWidth={1} />
              {/* p95 end cap */}
              <line x1={p95x} y1={cy - 4} x2={p95x} y2={cy + 4} stroke="#444" strokeWidth={1} />

              {/* IQR box: p25 to p75 */}
              <rect
                x={q1x}
                y={boxTop}
                width={boxW}
                height={BOX_H}
                fill={row.color}
                fillOpacity={0.25}
                stroke={row.color}
                strokeWidth={1}
              />

              {/* Median line at p50 */}
              <rect
                x={medx - medW / 2}
                y={boxTop}
                width={medW}
                height={BOX_H}
                fill={row.color}
              />

              {/* Mean text to the right */}
              <text
                x={LBL_W + PAD_LEFT + CHART_AREA + 6}
                y={cy}
                dominantBaseline="middle"
                fill={row.color}
                fontSize={8}
              >
                {fmt(row.mean, unit)}
              </text>
            </g>
          );
        })}
      </svg>

      {/* Tooltip */}
      {tooltip.visible && tooltip.group && (
        <div
          className="pointer-events-none absolute z-50 rounded border border-gray-700 bg-gray-900 px-2 py-1 text-xs text-gray-200"
          style={{
            left: tooltip.x,
            top: tooltip.y,
            fontFamily: 'var(--font-mono, ui-monospace, monospace)',
            whiteSpace: 'nowrap',
          }}
        >
          <span className="font-semibold" style={{ color: tooltip.group.color }}>
            {tooltip.group.label}
          </span>
          {': '}
          p5={fmt(tooltip.group.p5, unit)}{' '}
          p25={fmt(tooltip.group.p25, unit)}{' '}
          p50={fmt(tooltip.group.p50, unit)}{' '}
          p75={fmt(tooltip.group.p75, unit)}{' '}
          p95={fmt(tooltip.group.p95, unit)}{' '}
          mean={fmt(tooltip.group.mean, unit)}
        </div>
      )}
    </div>
  );
}
