import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { TOOLTIP_STYLE } from '../lib/chart';

interface ValueChartDatum {
  name: string;
  value: number | null;
}

interface ValueReportChartProps {
  data: ValueChartDatum[];
  higherIsBetter: boolean;
}

/** Feature-specific chart kept separate so report tables do not eagerly load Recharts. */
export default function ValueReportChart({ data, higherIsBetter }: ValueReportChartProps) {
  return (
    <div className="border border-gray-800 rounded p-4 mb-6">
      <p className="text-xs text-gray-400 mb-2">
        {higherIsBetter
          ? 'Sustained Mbps per dollar-hour (higher is better)'
          : 'Dollar-weighted p95 latency (lower is better)'}
      </p>
      <ResponsiveContainer width="100%" height={260}>
        <BarChart data={data} margin={{ top: 8, right: 16, left: 0, bottom: 8 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="#1a1b25" />
          <XAxis dataKey="name" tick={{ fill: '#788294', fontSize: 12 }} />
          <YAxis tick={{ fill: '#788294', fontSize: 12 }} />
          <Tooltip contentStyle={TOOLTIP_STYLE} cursor={{ fill: 'rgba(71,191,255,0.05)' }} />
          <Bar dataKey="value" fill="#47bfff" />
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}
