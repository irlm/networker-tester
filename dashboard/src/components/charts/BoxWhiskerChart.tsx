/**
 * Box-and-whisker chart — mirrors the SVG output from
 * crates/networker-tester/src/output/html.rs::svg_boxplot
 *
 * Horizontal layout: label | p5 ── [Q1 ▎median▐ Q3] ── p95 | annotation
 */

import { useMemo } from 'react';

export interface BoxGroup {
  label: string;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  color?: string;
}

interface BoxWhiskerChartProps {
  groups: BoxGroup[];
  unit?: string;
  title?: string;
}

// Palette — cycle through these for each group
const PALETTE = [
  '#94a3b8', // slate-400
  '#eab308', // yellow-500
  '#22d3ee', // cyan-400
  '#a78bfa', // violet-400
  '#f87171', // red-400
  '#34d399', // emerald-400
  '#fb923c', // orange-400
  '#818cf8', // indigo-400
];

function fmt(v: number): string {
  if (v >= 1000) return `${(v / 1000).toFixed(1)}s`;
  if (v >= 100) return `${Math.round(v)}ms`;
  if (v >= 10) return `${v.toFixed(1)}ms`;
  if (v >= 1) return `${v.toFixed(2)}ms`;
  return `${(v * 1000).toFixed(0)}µs`;
}

export function BoxWhiskerChart({ groups, unit = 'ms', title }: BoxWhiskerChartProps) {
  const rows = useMemo(() => groups.filter(g => g.p95 > 0), [groups]);

  if (rows.length === 0) return null;

  const LBL_W = 120;
  const BOX_AREA = 320;
  const ANN_W = 160;
  const ROW_H = 32;
  const BOX_H = 18;
  const PAD_TOP = title ? 28 : 8;
  const PAD_BOT = 12;

  const totalW = LBL_W + BOX_AREA + ANN_W;
  const totalH = PAD_TOP + rows.length * ROW_H + PAD_BOT;

  // Global scale from p5 to p95 with 5% padding
  const globalMin = Math.min(...rows.map(r => r.p5));
  const globalMax = Math.max(...rows.map(r => r.p95));
  const range = Math.max(globalMax - globalMin, 0.001);
  const pad = range * 0.05;
  const xLo = Math.max(globalMin - pad, 0);
  const xHi = globalMax + pad;
  const span = xHi - xLo;
  const scale = (v: number) => LBL_W + ((v - xLo) / span) * BOX_AREA;

  return (
    <div className="overflow-x-auto">
      <svg
        width={totalW}
        height={totalH}
        style={{ fontFamily: 'var(--font-mono, ui-monospace, monospace)', fontSize: 11 }}
      >
        {title && (
          <text x={LBL_W + 5} y={18} fontWeight="bold" fontSize={12} fill="#9ca3af">
            {title}
          </text>
        )}
        {rows.map((row, i) => {
          const color = row.color || PALETTE[i % PALETTE.length];
          const y0 = PAD_TOP + i * ROW_H;
          const cy = y0 + ROW_H / 2;
          const boxTop = cy - BOX_H / 2;

          const p5x = scale(row.p5);
          const q1x = scale(row.p25);
          const medx = scale(row.p50);
          const q3x = scale(row.p75);
          const p95x = scale(row.p95);
          const boxW = Math.max(q3x - q1x, 2);

          return (
            <g key={row.label}>
              {/* Label */}
              <text
                x={LBL_W - 8}
                y={cy}
                textAnchor="end"
                dominantBaseline="middle"
                fill="#9ca3af"
                fontSize={11}
              >
                {row.label}
              </text>

              {/* Dashed whisker line p5 → p95 */}
              <line
                x1={p5x}
                y1={cy}
                x2={p95x}
                y2={cy}
                stroke={color}
                strokeWidth={1.5}
                strokeDasharray="3,2"
                opacity={0.5}
              />

              {/* p5 tick */}
              <line x1={p5x} y1={cy - 6} x2={p5x} y2={cy + 6} stroke={color} strokeWidth={1.5} opacity={0.6} />

              {/* p95 tick */}
              <line x1={p95x} y1={cy - 6} x2={p95x} y2={cy + 6} stroke={color} strokeWidth={1.5} opacity={0.6} />

              {/* IQR box Q1 → Q3 */}
              <rect
                x={q1x}
                y={boxTop}
                width={boxW}
                height={BOX_H}
                rx={2}
                fill={color}
                opacity={0.75}
              />

              {/* Median line */}
              <line
                x1={medx}
                y1={boxTop}
                x2={medx}
                y2={boxTop + BOX_H}
                stroke="white"
                strokeWidth={2.5}
              />

              {/* Annotation */}
              <text
                x={LBL_W + BOX_AREA + 8}
                y={cy}
                dominantBaseline="middle"
                fill="#6b7280"
                fontSize={10}
              >
                p50={fmt(row.p50)}  p95={fmt(row.p95)} {unit}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}
