/**
 * Shared analysis module — mirrors the statistics logic from
 * crates/networker-tester/src/metrics.rs (compute_stats, primary_metric_value)
 * and crates/networker-tester/src/output/html.rs (protocol comparison tables).
 *
 * This is the single source of truth for dashboard statistics computation.
 * Both RunDetailPage and future comparison views should use these functions.
 */

import type { LiveAttempt } from '../api/types';

// ─── Stats ───────────────────────────────────────────────────────────────────

export interface Stats {
  count: number;
  min: number;
  mean: number;
  p5: number;
  p25: number;
  p50: number;
  p75: number;
  p95: number;
  p99: number;
  max: number;
  stddev: number;
}

/** Mirrors metrics.rs::compute_stats — percentiles via linear interpolation. */
export function computeStats(values: number[]): Stats | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const n = sorted.length;
  const min = sorted[0];
  const max = sorted[n - 1];
  const mean = sorted.reduce((a, b) => a + b, 0) / n;
  const p5 = percentile(sorted, 5);
  const p25 = percentile(sorted, 25);
  const p50 = percentile(sorted, 50);
  const p75 = percentile(sorted, 75);
  const p95 = percentile(sorted, 95);
  const p99 = percentile(sorted, 99);
  const variance = sorted.reduce((s, v) => s + (v - mean) ** 2, 0) / n;
  const stddev = Math.sqrt(variance);
  return { count: n, min, mean, p5, p25, p50, p75, p95, p99, max, stddev };
}

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 1) return sorted[0];
  const rank = (p / 100) * (sorted.length - 1);
  const lo = Math.floor(rank);
  const hi = Math.ceil(rank);
  if (lo === hi) return sorted[lo];
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo);
}

// ─── Primary metric extraction (mirrors metrics.rs::primary_metric_value) ────

export function primaryMetricValue(a: LiveAttempt): number | null {
  switch (a.protocol) {
    case 'http1': case 'http2': case 'http3':
    case 'native': case 'curl':
      return a.http?.total_duration_ms ?? null;
    case 'tcp':
      return a.tcp?.connect_duration_ms ?? null;
    case 'udp':
      return a.udp?.rtt_avg_ms ?? null;
    case 'dns':
      return a.dns?.duration_ms ?? null;
    case 'tls':
      return a.tls?.handshake_duration_ms ?? null;
    case 'download': case 'download1': case 'download2': case 'download3':
    case 'upload': case 'upload1': case 'upload2': case 'upload3':
    case 'webdownload': case 'webupload':
      return a.http?.throughput_mbps ?? null;
    case 'pageload': case 'pageload1': case 'pageload2': case 'pageload3':
      return a.page_load?.total_ms ?? null;
    case 'browser': case 'browser1': case 'browser2': case 'browser3':
      return a.browser?.load_ms ?? null;
    default:
      return a.http?.total_duration_ms ?? null;
  }
}

export function primaryMetricLabel(protocol: string): string {
  switch (protocol) {
    case 'tcp': return 'Connect ms';
    case 'udp': return 'RTT avg ms';
    case 'dns': return 'Resolve ms';
    case 'tls': return 'Handshake ms';
    case 'download': case 'download1': case 'download2': case 'download3':
    case 'upload': case 'upload1': case 'upload2': case 'upload3':
    case 'webdownload': case 'webupload':
      return 'Throughput MB/s';
    case 'pageload': case 'pageload1': case 'pageload2': case 'pageload3':
      return 'Total ms';
    case 'browser': case 'browser1': case 'browser2': case 'browser3':
      return 'Load ms';
    default:
      return 'Total ms';
  }
}

export function isThroughputProtocol(protocol: string): boolean {
  return [
    'download', 'download1', 'download2', 'download3',
    'upload', 'upload1', 'upload2', 'upload3',
    'webdownload', 'webupload', 'udpdownload', 'udpupload',
  ].includes(protocol);
}

/** For throughput protocols, higher is better. For latency, lower is better. */
export function isHigherBetter(protocol: string): boolean {
  return isThroughputProtocol(protocol);
}

// ─── Payload bytes extraction (mirrors metrics.rs::attempt_payload_bytes) ────

export function attemptPayloadBytes(a: LiveAttempt): number | null {
  if (isThroughputProtocol(a.protocol)) {
    return a.http?.payload_bytes ?? null;
  }
  return null;
}

// ─── Phase timing extraction ─────────────────────────────────────────────────

export interface PhaseTiming {
  dns_ms: number | null;
  tcp_ms: number | null;
  tls_ms: number | null;
  ttfb_ms: number | null;
  total_ms: number | null;
}

export function extractPhaseTiming(a: LiveAttempt): PhaseTiming {
  return {
    dns_ms: a.dns?.duration_ms ?? null,
    tcp_ms: a.tcp?.connect_duration_ms ?? null,
    tls_ms: a.tls?.handshake_duration_ms ?? null,
    ttfb_ms: a.http?.ttfb_ms ?? null,
    total_ms: a.http?.total_duration_ms ?? null,
  };
}

// ─── Grouping helpers ────────────────────────────────────────────────────────

/** Group attempts by protocol. */
export function groupByProtocol(attempts: LiveAttempt[]): Map<string, LiveAttempt[]> {
  const groups = new Map<string, LiveAttempt[]>();
  for (const a of attempts) {
    const key = a.protocol;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(a);
  }
  return groups;
}

/** Group attempts by (protocol, payloadBytes) for throughput. */
export function groupByProtocolAndPayload(
  attempts: LiveAttempt[]
): Map<string, LiveAttempt[]> {
  const groups = new Map<string, LiveAttempt[]>();
  for (const a of attempts) {
    const payload = attemptPayloadBytes(a);
    const key = payload != null ? `${a.protocol}:${payload}` : a.protocol;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push(a);
  }
  return groups;
}

// ─── Protocol stats summary (mirrors html.rs statistics summary table) ───────

export interface ProtocolStats {
  protocol: string;
  payloadBytes: number | null;
  label: string;
  stats: Stats;
  successRate: number; // 0-100
  totalAttempts: number;
  successCount: number;
}

/** Compute stats for each (protocol, payload) group. */
export function computeProtocolStats(attempts: LiveAttempt[]): ProtocolStats[] {
  const groups = groupByProtocolAndPayload(attempts.filter((a) => a.success));
  const allGroups = groupByProtocolAndPayload(attempts);
  const results: ProtocolStats[] = [];

  for (const [key, successAttempts] of groups) {
    const values = successAttempts
      .map(primaryMetricValue)
      .filter((v): v is number => v != null);
    const stats = computeStats(values);
    if (!stats) continue;

    const allForKey = allGroups.get(key) ?? [];
    const protocol = key.split(':')[0];
    const payloadStr = key.includes(':') ? key.split(':')[1] : null;
    const payloadBytes = payloadStr ? parseInt(payloadStr, 10) : null;

    results.push({
      protocol,
      payloadBytes,
      label: primaryMetricLabel(protocol),
      stats,
      successRate: (successAttempts.length / allForKey.length) * 100,
      totalAttempts: allForKey.length,
      successCount: successAttempts.length,
    });
  }

  return results;
}

// ─── Timing breakdown (mirrors html.rs append_proto_row) ─────────────────────

export interface TimingBreakdown {
  protocol: string;
  count: number;
  avgDns: number | null;
  avgTcp: number | null;
  avgTls: number | null;
  avgTtfb: number | null;
  avgTotal: number | null;
  successCount: number;
  totalCount: number;
}

export function computeTimingBreakdown(attempts: LiveAttempt[]): TimingBreakdown[] {
  const groups = groupByProtocol(attempts);
  const results: TimingBreakdown[] = [];

  for (const [protocol, group] of groups) {
    if (isThroughputProtocol(protocol)) continue; // throughput has its own section

    const successful = group.filter((a) => a.success);
    const timings = successful.map(extractPhaseTiming);

    const avg = (vals: (number | null)[]) => {
      const nums = vals.filter((v): v is number => v != null);
      return nums.length > 0 ? nums.reduce((a, b) => a + b, 0) / nums.length : null;
    };

    results.push({
      protocol,
      count: group.length,
      avgDns: avg(timings.map((t) => t.dns_ms)),
      avgTcp: avg(timings.map((t) => t.tcp_ms)),
      avgTls: avg(timings.map((t) => t.tls_ms)),
      avgTtfb: avg(timings.map((t) => t.ttfb_ms)),
      avgTotal: avg(timings.map((t) => t.total_ms)),
      successCount: successful.length,
      totalCount: group.length,
    });
  }

  return results;
}

// ─── Formatting helpers ──────────────────────────────────────────────────────

export function formatMs(ms: number | null | undefined): string {
  if (ms == null) return '-';
  if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`;
  if (ms < 100) return `${ms.toFixed(2)}ms`;
  return `${ms.toFixed(1)}ms`;
}

export function formatThroughput(mbps: number | null | undefined): string {
  if (mbps == null) return '-';
  if (mbps >= 1000) return `${(mbps / 1000).toFixed(1)} GB/s`;
  return `${mbps.toFixed(1)} MB/s`;
}

export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}

export function formatMetricValue(protocol: string, value: number | null): string {
  if (value == null) return '-';
  return isThroughputProtocol(protocol) ? formatThroughput(value) : formatMs(value);
}

export function successRateClass(rate: number): string {
  if (rate >= 100) return 'text-green-400';
  if (rate >= 80) return 'text-yellow-400';
  return 'text-red-400';
}
