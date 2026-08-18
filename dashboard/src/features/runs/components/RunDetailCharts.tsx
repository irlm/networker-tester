import {
  Bar,
  BarChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { TOOLTIP_STYLE } from '../../../lib/chart';

interface ProtocolChartDatum {
  name: string;
  p50: number;
  p95: number;
  mean: number;
}

interface TtfbDistributionDatum {
  range: string;
  count: number;
}

interface RunDetailChartsProps {
  protocolData: ProtocolChartDatum[];
  ttfbData: TtfbDistributionDatum[];
}

/**
 * Recharts-backed visualizations are isolated behind a lazy page boundary.
 * Most run pages can render their tables and SVG box plot without downloading
 * the charting runtime; this module is requested only when either chart has data.
 */
export default function RunDetailCharts({ protocolData, ttfbData }: RunDetailChartsProps) {
  return (
    <>
      {protocolData.length > 1 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">
            protocol comparison — p50 vs p95
          </h3>
          <ResponsiveContainer width="100%" height={250}>
            <BarChart data={protocolData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis dataKey="name" stroke="#788294" fontSize={12} />
              <YAxis stroke="#788294" fontSize={12} />
              <Tooltip contentStyle={TOOLTIP_STYLE} />
              <Bar dataKey="p50" fill="#94a3b8" name="p50" />
              <Bar dataKey="p95" fill="#eab308" name="p95" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {ttfbData.length > 0 && (
        <div className="mb-6">
          <h3 className="text-xs text-gray-400 tracking-wider mb-3 font-medium">TTFB distribution (ms)</h3>
          <ResponsiveContainer width="100%" height={220}>
            <BarChart data={ttfbData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis
                dataKey="range"
                stroke="#788294"
                fontSize={12}
                angle={-30}
                textAnchor="end"
                height={64}
              />
              <YAxis stroke="#788294" fontSize={12} allowDecimals={false} />
              <Tooltip contentStyle={TOOLTIP_STYLE} />
              <Bar dataKey="count" fill="#9b74e8" name="Attempts" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}
    </>
  );
}
