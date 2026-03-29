import { useState, useMemo } from 'react';
import { PageHeader } from '../components/common/PageHeader';
import { usePageTitle } from '../hooks/usePageTitle';

// ── Types ──────────────────────────────────────────────────────────────

interface BenchmarkResult {
  language: string;
  runtime: string;
  mean: number;
  p50: number;
  p95: number;
  p99: number;
  cpu: number;
  memory: number;
  coldStart: number;
  binarySize: number;
}

type Tab = 'leaderboard' | 'comparison' | 'timeline';
type SortKey = keyof BenchmarkResult;
type SortDir = 'asc' | 'desc';

// ── Sample Data ────────────────────────────────────────────────────────

const SAMPLE_RESULTS: BenchmarkResult[] = [
  { language: 'Rust', runtime: 'hyper', mean: 0.06, p50: 0.09, p95: 0.13, p99: 0.22, cpu: 5, memory: 18, coldStart: 15, binarySize: 11 },
  { language: 'Go', runtime: 'net/http', mean: 0.06, p50: 0.07, p95: 0.09, p99: 0.13, cpu: 8, memory: 24, coldStart: 12, binarySize: 7 },
  { language: 'Python', runtime: 'uvicorn', mean: 0.17, p50: 0.21, p95: 0.22, p99: 0.30, cpu: 15, memory: 45, coldStart: 800, binarySize: 0 },
  { language: 'Java', runtime: 'HttpServer', mean: 0.24, p50: 0.41, p95: 0.48, p99: 0.90, cpu: 22, memory: 120, coldStart: 1200, binarySize: 0 },
  { language: 'C# .NET 10', runtime: 'Kestrel', mean: 0.10, p50: 0.08, p95: 0.12, p99: 0.25, cpu: 10, memory: 65, coldStart: 200, binarySize: 45 },
];

// ── Helpers ────────────────────────────────────────────────────────────

function rankColor(rank: number): string {
  if (rank === 1) return 'text-yellow-400';
  if (rank === 2) return 'text-gray-300';
  if (rank === 3) return 'text-amber-600';
  return 'text-gray-500';
}

function rankBg(rank: number): string {
  if (rank === 1) return 'bg-yellow-400/5';
  if (rank === 2) return 'bg-gray-300/5';
  if (rank === 3) return 'bg-amber-600/5';
  return '';
}

function formatMemory(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${mb} MB`;
}

function formatColdStart(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${ms}ms`;
}

function formatBinarySize(mb: number): string {
  if (mb === 0) return '\u2014';
  return `${mb} MB`;
}

// ── Bar component for comparison charts ────────────────────────────────

function Bar({ value, max, color, label }: { value: number; max: number; color: string; label: string }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  return (
    <div className="flex items-center gap-2 mb-1">
      <span className="text-xs text-gray-500 w-8 text-right font-mono">{label}</span>
      <div className="flex-1 h-4 bg-gray-800 rounded overflow-hidden">
        <div
          className={`h-full rounded transition-all duration-300 ${color}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="text-xs text-gray-400 w-16 text-right font-mono">{value.toFixed(2)}</span>
    </div>
  );
}

function ResourceBar({ value, max, color, label, unit }: { value: number; max: number; color: string; label: string; unit: string }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  return (
    <div className="flex items-center gap-2 mb-1">
      <span className="text-xs text-gray-500 w-8 text-right font-mono">{label}</span>
      <div className="flex-1 h-4 bg-gray-800 rounded overflow-hidden">
        <div
          className={`h-full rounded transition-all duration-300 ${color}`}
          style={{ width: `${pct}%` }}
        />
      </div>
      <span className="text-xs text-gray-400 w-20 text-right font-mono">{value}{unit}</span>
    </div>
  );
}

// ── Sort arrow indicator ───────────────────────────────────────────────

function SortArrow({ active, dir }: { active: boolean; dir: SortDir }) {
  if (!active) return <span className="text-gray-700 ml-0.5">{'\u2195'}</span>;
  return <span className="text-cyan-400 ml-0.5">{dir === 'asc' ? '\u2191' : '\u2193'}</span>;
}

// ── Leaderboard Tab ────────────────────────────────────────────────────

function LeaderboardTab({ data }: { data: BenchmarkResult[] }) {
  const [sortKey, setSortKey] = useState<SortKey>('mean');
  const [sortDir, setSortDir] = useState<SortDir>('asc');

  const sorted = useMemo(() => {
    const copy = [...data];
    copy.sort((a, b) => {
      const av = a[sortKey];
      const bv = b[sortKey];
      if (typeof av === 'number' && typeof bv === 'number') {
        return sortDir === 'asc' ? av - bv : bv - av;
      }
      return sortDir === 'asc'
        ? String(av).localeCompare(String(bv))
        : String(bv).localeCompare(String(av));
    });
    return copy;
  }, [data, sortKey, sortDir]);

  const handleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDir(d => d === 'asc' ? 'desc' : 'asc');
    } else {
      setSortKey(key);
      setSortDir('asc');
    }
  };

  const th = (label: string, key: SortKey) => (
    <th
      className="pb-2 pr-3 font-medium cursor-pointer select-none hover:text-gray-300 transition-colors"
      onClick={() => handleSort(key)}
    >
      <span className="inline-flex items-center">
        {label}
        <SortArrow active={sortKey === key} dir={sortDir} />
      </span>
    </th>
  );

  return (
    <div className="table-container">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-left text-xs text-gray-500 border-b border-gray-800">
            <th className="pb-2 pr-3 font-medium w-12">#</th>
            {th('Language', 'language')}
            {th('Runtime', 'runtime')}
            {th('Mean (ms)', 'mean')}
            {th('p50', 'p50')}
            {th('p95', 'p95')}
            {th('p99', 'p99')}
            {th('CPU%', 'cpu')}
            {th('Memory', 'memory')}
            {th('Cold Start', 'coldStart')}
            {th('Binary', 'binarySize')}
          </tr>
        </thead>
        <tbody>
          {sorted.map((r, i) => {
            const rank = i + 1;
            return (
              <tr key={r.language} className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${rankBg(rank)}`}>
                <td className={`py-2 pr-3 font-mono font-bold ${rankColor(rank)}`}>
                  {rank}
                </td>
                <td className="py-2 pr-3 text-gray-200 font-mono font-medium">{r.language}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{r.runtime}</td>
                <td className="py-2 pr-3 text-cyan-400 font-mono font-medium">{r.mean.toFixed(2)}</td>
                <td className="py-2 pr-3 text-gray-300 font-mono">{r.p50.toFixed(2)}</td>
                <td className="py-2 pr-3 text-gray-300 font-mono">{r.p95.toFixed(2)}</td>
                <td className="py-2 pr-3 text-gray-300 font-mono">{r.p99.toFixed(2)}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{r.cpu}%</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{formatMemory(r.memory)}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{formatColdStart(r.coldStart)}</td>
                <td className="py-2 pr-3 text-gray-400 font-mono">{formatBinarySize(r.binarySize)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

// ── Comparison Tab ─────────────────────────────────────────────────────

const COMPARISON_COLORS = [
  'bg-cyan-500',
  'bg-purple-500',
  'bg-emerald-500',
  'bg-amber-500',
];

function ComparisonTab({ data }: { data: BenchmarkResult[] }) {
  const [selected, setSelected] = useState<Set<string>>(() => new Set(['Rust', 'Go']));

  const toggle = (lang: string) => {
    setSelected(prev => {
      const next = new Set(prev);
      if (next.has(lang)) {
        next.delete(lang);
      } else if (next.size < 4) {
        next.add(lang);
      }
      return next;
    });
  };

  const items = data.filter(r => selected.has(r.language));

  const maxLatency = Math.max(...items.map(r => r.p99), 0.01);
  const maxCpu = Math.max(...items.map(r => r.cpu), 1);
  const maxMemory = Math.max(...items.map(r => r.memory), 1);

  return (
    <div>
      {/* Language selector */}
      <div className="flex items-center gap-3 mb-6 flex-wrap">
        <span className="text-xs text-gray-500">Select languages (max 4):</span>
        {data.map(r => (
          <button
            key={r.language}
            onClick={() => toggle(r.language)}
            className={`px-3 py-1.5 text-xs rounded border transition-colors ${
              selected.has(r.language)
                ? 'border-cyan-600 text-cyan-400 bg-cyan-500/10'
                : 'border-gray-700 text-gray-400 hover:text-gray-200 hover:border-gray-600'
            }`}
          >
            {r.language}
          </button>
        ))}
      </div>

      {items.length === 0 ? (
        <div className="text-center text-gray-600 py-12">Select at least one language to compare</div>
      ) : (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          {/* Latency comparison */}
          <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
            <div className="text-xs text-gray-500 tracking-wider font-medium mb-4">LATENCY (ms)</div>
            {items.map((r, idx) => (
              <div key={r.language} className="mb-4">
                <div className="text-xs text-gray-300 mb-1.5 font-mono">{r.language} <span className="text-gray-600">({r.runtime})</span></div>
                <Bar value={r.p50} max={maxLatency} color={COMPARISON_COLORS[idx % COMPARISON_COLORS.length]} label="p50" />
                <Bar value={r.p95} max={maxLatency} color={COMPARISON_COLORS[idx % COMPARISON_COLORS.length] + '/70'} label="p95" />
                <Bar value={r.p99} max={maxLatency} color={COMPARISON_COLORS[idx % COMPARISON_COLORS.length] + '/50'} label="p99" />
              </div>
            ))}
          </div>

          {/* Resource usage */}
          <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4">
            <div className="text-xs text-gray-500 tracking-wider font-medium mb-4">RESOURCE USAGE</div>
            {items.map((r, idx) => (
              <div key={r.language} className="mb-4">
                <div className="text-xs text-gray-300 mb-1.5 font-mono">{r.language} <span className="text-gray-600">({r.runtime})</span></div>
                <ResourceBar value={r.cpu} max={maxCpu} color={COMPARISON_COLORS[idx % COMPARISON_COLORS.length]} label="CPU" unit="%" />
                <ResourceBar value={r.memory} max={maxMemory} color={COMPARISON_COLORS[idx % COMPARISON_COLORS.length] + '/70'} label="Mem" unit=" MB" />
              </div>
            ))}
          </div>

          {/* Cold start + binary size */}
          <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-4 lg:col-span-2">
            <div className="text-xs text-gray-500 tracking-wider font-medium mb-4">STARTUP &amp; SIZE</div>
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
              {items.map((r, idx) => (
                <div key={r.language} className="text-center">
                  <div className={`text-xs mb-2 font-mono ${COMPARISON_COLORS[idx % COMPARISON_COLORS.length].replace('bg-', 'text-')}`}>
                    {r.language}
                  </div>
                  <div className="text-lg text-gray-200 font-mono font-bold">{formatColdStart(r.coldStart)}</div>
                  <div className="text-[10px] text-gray-600 uppercase tracking-wider">Cold start</div>
                  <div className="text-sm text-gray-300 font-mono mt-2">{formatBinarySize(r.binarySize)}</div>
                  <div className="text-[10px] text-gray-600 uppercase tracking-wider">Binary</div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ── Timeline Tab (placeholder) ─────────────────────────────────────────

function TimelineTab() {
  return (
    <div className="border border-gray-800 rounded bg-[var(--bg-card)] p-8 text-center">
      <div className="text-gray-600 text-4xl mb-4">{'\u23F3'}</div>
      <div className="text-gray-400 text-sm mb-2">Coming soon</div>
      <div className="text-gray-600 text-xs">Run benchmarks to populate historical comparison data</div>
    </div>
  );
}

// ── Main Page ──────────────────────────────────────────────────────────

export function BenchmarksPage() {
  usePageTitle('Benchmarks');
  const [tab, setTab] = useState<Tab>('leaderboard');

  const tabCls = (t: Tab) =>
    `px-4 py-2 text-sm rounded-t transition-colors ${
      tab === t
        ? 'bg-gray-800/40 text-gray-100'
        : 'text-gray-400 hover:text-gray-200'
    }`;

  return (
    <div className="p-4 md:p-6 max-w-7xl">
      <PageHeader
        title="Benchmarks"
        subtitle="AletheBench language runtime comparison"
      />

      {/* Tab selector */}
      <div className="flex gap-1 mb-4">
        <button onClick={() => setTab('leaderboard')} className={tabCls('leaderboard')}>Leaderboard</button>
        <button onClick={() => setTab('comparison')} className={tabCls('comparison')}>Comparison</button>
        <button onClick={() => setTab('timeline')} className={tabCls('timeline')}>Timeline</button>
      </div>

      {tab === 'leaderboard' && <LeaderboardTab data={SAMPLE_RESULTS} />}
      {tab === 'comparison' && <ComparisonTab data={SAMPLE_RESULTS} />}
      {tab === 'timeline' && <TimelineTab />}
    </div>
  );
}
