import {
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { TOOLTIP_STYLE } from '../lib/chart';
import type { ProbeComparisonPoint } from '../api/types';
import { colorFor, shortLabel, toChartRows } from '../lib/probeComparison';

interface ProbeCompareChartProps {
  points: ProbeComparisonPoint[];
  urls: string[];
  bucketSeconds: number;
}

function formatTick(ms: number, bucketSeconds: number): string {
  const d = new Date(ms);
  // A day-wide bucket labelled with a time of day reads as false precision.
  return bucketSeconds >= 86400
    ? d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
    : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

/**
 * Overlaid p50 latency per URL, one line each, over the compared window.
 *
 * Kept in its own module so the report tables never eagerly pull Recharts in
 * (pinned by e2e/lazy-charts.spec.ts). A bucket a URL did not measure is a
 * GAP in its line, never a zero — `connectNulls` stays off deliberately, so
 * the chart shows where the data actually is and the shared-bucket rule the
 * scoreboard applies is visible rather than merely asserted.
 */
export default function ProbeCompareChart({ points, urls, bucketSeconds }: ProbeCompareChartProps) {
  const rows = toChartRows(points, urls);

  return (
    <div className="border border-gray-800 rounded p-4 mb-6">
      <p className="text-xs text-gray-400 mb-2">
        Median response time per bucket (lower is better). A gap is a bucket that URL was not
        measured in — those buckets count for nobody.
      </p>
      <ResponsiveContainer width="100%" height={280}>
        <LineChart data={rows} margin={{ top: 8, right: 16, left: 0, bottom: 8 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="#1a1b25" />
          <XAxis
            dataKey="bucket"
            type="number"
            domain={['dataMin', 'dataMax']}
            scale="time"
            tick={{ fill: '#788294', fontSize: 12 }}
            tickFormatter={(v: number) => formatTick(v, bucketSeconds)}
          />
          <YAxis
            tick={{ fill: '#788294', fontSize: 12 }}
            label={{ value: 'ms', angle: -90, position: 'insideLeft', fill: '#788294', fontSize: 12 }}
          />
          <Tooltip
            contentStyle={TOOLTIP_STYLE}
            labelFormatter={(v) => new Date(Number(v)).toLocaleString()}
            formatter={(value, name) => [
              typeof value === 'number' ? `${value.toFixed(1)} ms` : '—',
              name,
            ]}
          />
          <Legend wrapperStyle={{ fontSize: 12 }} />
          {urls.map((url) => (
            <Line
              key={url}
              type="monotone"
              dataKey={`p50:${url}`}
              name={shortLabel(url)}
              stroke={colorFor(urls, url)}
              strokeWidth={2}
              dot={false}
              connectNulls={false}
              isAnimationActive={false}
            />
          ))}
        </LineChart>
      </ResponsiveContainer>
    </div>
  );
}
