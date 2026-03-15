import { useState, useMemo } from 'react';
import { useParams } from 'react-router-dom';
import { api, type Attempt, type RunSummary } from '../api/client';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  CartesianGrid,
} from 'recharts';

interface AttemptDetail extends Attempt {
  dns?: { duration_ms: number; resolved_ips: string[] };
  tcp?: { connect_duration_ms: number; remote_addr: string };
  tls?: { handshake_duration_ms: number; protocol_version: string; cipher_suite: string };
  http?: { status_code: number; ttfb_ms: number; total_duration_ms: number; negotiated_version: string; throughput_mbps?: number };
  error?: { category: string; message: string };
}

export function RunDetailPage() {
  const { runId } = useParams<{ runId: string }>();
  const [run, setRun] = useState<RunSummary | null>(null);
  const [attempts, setAttempts] = useState<AttemptDetail[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedProtocols, setExpandedProtocols] = useState<Set<string>>(new Set());

  const shortId = runId?.slice(0, 8) ?? '';
  usePageTitle(runId ? `Run ${shortId}` : 'Run');

  // Fetch run metadata from the runs list
  usePolling(
    () => {
      if (!runId) return;
      // Fetch attempts (primary data)
      api
        .getRunAttempts(runId)
        .then((data) => {
          setAttempts(data as AttemptDetail[]);
          setError(null);
          setLoading(false);
        })
        .catch((e) => {
          setError(String(e));
          setLoading(false);
        });
      // Also fetch run summary for metadata
      api
        .getRuns({ limit: 200 })
        .then((runs) => {
          const found = runs.find((r) => r.run_id === runId);
          if (found) setRun(found);
        })
        .catch(() => {
          // Non-critical, ignore
        });
    },
    15000,
    !!runId
  );

  // Group attempts by protocol
  const groupedByProtocol = useMemo(() => {
    const groups: Record<string, AttemptDetail[]> = {};
    for (const a of attempts) {
      const key = a.protocol || 'unknown';
      if (!groups[key]) groups[key] = [];
      groups[key].push(a);
    }
    return groups;
  }, [attempts]);

  // TTFB distribution chart data
  const ttfbChartData = useMemo(() => {
    const withTtfb = attempts.filter((a) => a.http?.ttfb_ms != null);
    if (withTtfb.length === 0) return [];

    // Create histogram buckets
    const values = withTtfb.map((a) => a.http!.ttfb_ms);
    const min = Math.floor(Math.min(...values));
    const max = Math.ceil(Math.max(...values));
    const range = max - min || 1;
    const bucketCount = Math.min(20, Math.max(5, Math.ceil(values.length / 3)));
    const bucketSize = range / bucketCount;

    const buckets: { range: string; count: number; from: number }[] = [];
    for (let i = 0; i < bucketCount; i++) {
      const from = min + i * bucketSize;
      const to = from + bucketSize;
      buckets.push({
        range: `${from.toFixed(0)}-${to.toFixed(0)}`,
        count: 0,
        from,
      });
    }

    for (const v of values) {
      const idx = Math.min(
        Math.floor((v - min) / bucketSize),
        bucketCount - 1
      );
      buckets[idx].count++;
    }

    return buckets;
  }, [attempts]);

  const toggleProtocol = (protocol: string) => {
    setExpandedProtocols((prev) => {
      const next = new Set(prev);
      if (next.has(protocol)) {
        next.delete(protocol);
      } else {
        next.add(protocol);
      }
      return next;
    });
  };

  if (loading && attempts.length === 0) {
    return (
      <div className="p-6">
        <Breadcrumb items={[{ label: 'Runs', to: '/runs' }, { label: `Run ${shortId}` }]} />
        <div className="text-gray-500 motion-safe:animate-pulse">
          Loading run {shortId}...
        </div>
      </div>
    );
  }

  if (error && attempts.length === 0) {
    return (
      <div className="p-6">
        <Breadcrumb items={[{ label: 'Runs', to: '/runs' }, { label: `Run ${shortId}` }]} />
        <div className="bg-red-500/10 border border-red-500/30 rounded-lg p-4">
          <h3 className="text-red-400 font-bold mb-2">Failed to load run</h3>
          <p className="text-red-300 text-sm font-mono">{error}</p>
          <p className="text-gray-500 text-xs mt-2">Run ID: {runId}</p>
        </div>
      </div>
    );
  }

  const successCount = attempts.filter((a) => a.success).length;
  const failureCount = attempts.filter((a) => !a.success).length;

  return (
    <div className="p-6">
      <Breadcrumb items={[{ label: 'Runs', to: '/runs' }, { label: `Run ${shortId}` }]} />

      {/* Run metadata header */}
      <div className="flex items-center justify-between mb-6">
        <div>
          <h2 className="text-xl font-bold text-gray-100 mb-1">
            Run {shortId}
          </h2>
          <p className="text-sm text-gray-500">
            {run?.target_host && <>Target: {run.target_host} | </>}
            {run?.modes && <>Modes: {run.modes} | </>}
            {attempts.length} attempts
          </p>
        </div>
      </div>

      {/* Summary cards */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 uppercase tracking-wider mb-1">Total</p>
          <p className="text-2xl font-bold text-cyan-400">{attempts.length}</p>
        </div>
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 uppercase tracking-wider mb-1">Success</p>
          <p className="text-2xl font-bold text-green-400">{successCount}</p>
        </div>
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 uppercase tracking-wider mb-1">Failed</p>
          <p className="text-2xl font-bold text-red-400">{failureCount}</p>
        </div>
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 uppercase tracking-wider mb-1">Started</p>
          <p className="text-lg font-bold text-gray-300">
            {run?.started_at ? new Date(run.started_at).toLocaleTimeString() : '-'}
          </p>
        </div>
      </div>

      {error && (
        <div className="bg-yellow-500/10 border border-yellow-500/30 rounded-lg p-3 mb-4 text-yellow-400 text-sm">
          Failed to refresh: {error}
        </div>
      )}

      {/* TTFB Distribution Chart */}
      {ttfbChartData.length > 0 && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4 mb-6">
          <h3 className="text-sm text-gray-400 mb-3">TTFB Distribution (ms)</h3>
          <ResponsiveContainer width="100%" height={220}>
            <BarChart data={ttfbChartData}>
              <CartesianGrid strokeDasharray="3 3" stroke="#1f2028" />
              <XAxis dataKey="range" stroke="#4b5563" fontSize={10} angle={-30} textAnchor="end" height={50} />
              <YAxis stroke="#4b5563" fontSize={10} allowDecimals={false} />
              <Tooltip
                contentStyle={{
                  background: '#12131a',
                  border: '1px solid #374151',
                  borderRadius: 6,
                  fontSize: 12,
                }}
              />
              <Bar dataKey="count" fill="#06b6d4" name="Attempts" />
            </BarChart>
          </ResponsiveContainer>
        </div>
      )}

      {/* Attempts grouped by protocol */}
      {Object.entries(groupedByProtocol).map(([protocol, protocolAttempts]) => {
        const isExpanded = expandedProtocols.has(protocol);
        const protoSuccess = protocolAttempts.filter((a) => a.success).length;
        const protoFail = protocolAttempts.length - protoSuccess;

        return (
          <div
            key={protocol}
            className="bg-[#12131a] border border-gray-800 rounded-lg mb-4 overflow-hidden"
          >
            <button
              onClick={() => toggleProtocol(protocol)}
              className="w-full px-4 py-3 flex items-center justify-between text-left hover:bg-gray-800/20 transition-colors"
              aria-expanded={isExpanded}
            >
              <div className="flex items-center gap-3">
                <span
                  className="text-gray-500 text-xs transition-transform"
                  style={{ transform: isExpanded ? 'rotate(90deg)' : 'rotate(0deg)' }}
                  aria-hidden="true"
                >
                  {'\u25B6'}
                </span>
                <span className="text-gray-200 font-medium text-sm">
                  {protocol.toUpperCase()}
                </span>
                <span className="text-gray-500 text-xs">
                  {protocolAttempts.length} attempts
                </span>
              </div>
              <div className="flex items-center gap-3 text-xs">
                <span className="text-green-400">{protoSuccess} OK</span>
                {protoFail > 0 && (
                  <span className="text-red-400">{protoFail} FAIL</span>
                )}
              </div>
            </button>

            {isExpanded && (
              <div className="border-t border-gray-800">
                <div className="max-h-96 overflow-y-auto">
                  {protocolAttempts.map((a) => (
                    <div
                      key={a.attempt_id}
                      className="px-4 py-3 border-b border-gray-800/30 hover:bg-gray-800/10"
                    >
                      {/* Attempt header row */}
                      <div className="flex items-center gap-4 mb-2">
                        <span className="text-gray-500 font-mono text-xs w-8">
                          #{a.sequence_num}
                        </span>
                        {a.success ? (
                          <span className="text-green-400 text-xs font-medium">OK</span>
                        ) : (
                          <span className="text-red-400 text-xs font-medium">FAIL</span>
                        )}
                        <span className="text-gray-600 text-xs">
                          {a.retry_count > 0 && `${a.retry_count} retries`}
                        </span>
                      </div>

                      {/* Sub-results */}
                      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-2 text-xs">
                        {a.dns && (
                          <div className="bg-[#0a0b0f] rounded p-2">
                            <p className="text-gray-500 uppercase tracking-wider mb-1">DNS</p>
                            <p className="text-gray-300">
                              {a.dns.duration_ms.toFixed(1)}ms
                            </p>
                            {a.dns.resolved_ips.length > 0 && (
                              <p className="text-gray-500 font-mono truncate">
                                {a.dns.resolved_ips.join(', ')}
                              </p>
                            )}
                          </div>
                        )}

                        {a.tcp && (
                          <div className="bg-[#0a0b0f] rounded p-2">
                            <p className="text-gray-500 uppercase tracking-wider mb-1">TCP</p>
                            <p className="text-gray-300">
                              {a.tcp.connect_duration_ms.toFixed(1)}ms
                            </p>
                            <p className="text-gray-500 font-mono truncate">
                              {a.tcp.remote_addr}
                            </p>
                          </div>
                        )}

                        {a.tls && (
                          <div className="bg-[#0a0b0f] rounded p-2">
                            <p className="text-gray-500 uppercase tracking-wider mb-1">TLS</p>
                            <p className="text-gray-300">
                              {a.tls.handshake_duration_ms.toFixed(1)}ms
                            </p>
                            <p className="text-gray-500 font-mono truncate">
                              {a.tls.protocol_version} {a.tls.cipher_suite}
                            </p>
                          </div>
                        )}

                        {a.http && (
                          <div className="bg-[#0a0b0f] rounded p-2">
                            <p className="text-gray-500 uppercase tracking-wider mb-1">HTTP</p>
                            <p className="text-gray-300">
                              {a.http.status_code} | TTFB {a.http.ttfb_ms.toFixed(1)}ms | Total {a.http.total_duration_ms.toFixed(1)}ms
                            </p>
                            <p className="text-gray-500 font-mono truncate">
                              {a.http.negotiated_version}
                              {a.http.throughput_mbps != null && ` | ${a.http.throughput_mbps.toFixed(2)} Mbps`}
                            </p>
                          </div>
                        )}

                        {a.error && (
                          <div className="bg-red-500/5 rounded p-2 border border-red-500/20">
                            <p className="text-red-400 uppercase tracking-wider mb-1">Error</p>
                            <p className="text-red-300">{a.error.category}</p>
                            <p className="text-red-400/70 truncate">{a.error.message}</p>
                          </div>
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        );
      })}

      {attempts.length === 0 && (
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-6 text-center">
          <p className="text-gray-600 text-sm">No attempts recorded for this run.</p>
        </div>
      )}
    </div>
  );
}
