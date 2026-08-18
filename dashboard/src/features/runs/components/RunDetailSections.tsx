import type { ReactNode } from 'react';
import type { BenchmarkArtifact, LiveAttempt } from '../../../api/types';
import {
  formatBytes,
  formatMetricValue,
  formatMs,
  successRateClass,
  type ProtocolStats,
  type TimingBreakdown,
} from '../../../lib/analysis';

export function ArtifactSection({ artifact }: { artifact: BenchmarkArtifact }) {
  return (
    <div className="artifact-section mt-8 pt-6">
      <h3 className="artifact-heading text-sm font-bold mb-4 flex items-center gap-2">
        <span className="artifact-symbol">&#9670;</span> Benchmark Artifact
      </h3>

      {/* Data Quality Summary */}
      {artifact.data_quality && (
        <div className="border border-gray-800 rounded p-4 mb-4 text-xs">
          <div className="flex flex-wrap gap-x-6 gap-y-2">
            <span className="text-gray-400">
              Noise: <span className={artifact.data_quality.noise_level === 'low' ? 'text-green-400' : 'text-yellow-400'}>
                {artifact.data_quality.noise_level}
              </span>
            </span>
            <span className="text-gray-400">
              {/* Artifact vocabulary is adequate/marginal/insufficient
                  (json.rs) — this compared against 'sufficient' and painted
                  every healthy run yellow. */}
              Sufficiency: <span className={artifact.data_quality.sufficiency === 'adequate' ? 'text-green-400' : 'text-yellow-400'}>
                {artifact.data_quality.sufficiency}
              </span>
            </span>
            <span className="text-gray-400">
              Publication: <span className={artifact.data_quality.publication_ready ? 'text-green-400' : 'text-red-400'}>
                {artifact.data_quality.publication_ready ? 'Ready' : 'Not Ready'}
              </span>
            </span>
            {artifact.data_quality.quality_tier && (
              <span className="text-gray-400">
                Tier: <span className="text-gray-300">{artifact.data_quality.quality_tier}</span>
              </span>
            )}
          </div>
          {(artifact.data_quality.warnings?.length ?? 0) > 0 && (
            <div className="mt-2 space-y-1">
              {artifact.data_quality.warnings.map((w, i) => (
                <p key={i} className="text-yellow-400/80">&#9888; {w}</p>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Case Summaries */}
      {artifact.summaries && artifact.summaries.length > 0 && (
        <div className="table-container mb-4">
          <h4 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
            case summaries
          </h4>
          <div className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead>
                <tr className="border-b border-gray-800 text-gray-400">
                  <th className="px-4 py-2 text-left">Protocol</th>
                  <th className="px-4 py-2 text-left">Metric</th>
                  <th className="px-4 py-2 text-right">N</th>
                  <th className="px-4 py-2 text-right">p50</th>
                  <th className="px-4 py-2 text-right">p95</th>
                  <th className="px-4 py-2 text-right">p99</th>
                  <th className="px-4 py-2 text-right">RPS</th>
                  <th className="px-4 py-2 text-right">StdDev</th>
                </tr>
              </thead>
              <tbody>
                {(Array.isArray(artifact.summaries) ? artifact.summaries : [artifact.summaries]).map((s, i) => (
                  <tr key={i} className="border-b border-gray-800/30 hover:bg-gray-800/10">
                    <td className="px-4 py-2 text-gray-200">{s.protocol}</td>
                    <td className="px-4 py-2 text-gray-400">{s.metric_name} ({s.metric_unit})</td>
                    <td className="px-4 py-2 text-gray-400 text-right">{s.included_sample_count}</td>
                    <td className="px-4 py-2 text-gray-100 text-right">{s.p50.toFixed(2)}</td>
                    <td className="px-4 py-2 text-yellow-400 text-right">{s.p95.toFixed(2)}</td>
                    <td className="px-4 py-2 text-s4 text-right">{s.p99.toFixed(2)}</td>
                    <td className="px-4 py-2 text-gray-300 text-right">{s.rps.toFixed(0)}</td>
                    <td className="px-4 py-2 text-gray-400 text-right">{s.stddev.toFixed(2)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Methodology */}
      {artifact.methodology && (
        <div className="border border-gray-800 rounded p-4 text-xs text-gray-400 space-y-1">
          <p className="text-gray-400 font-medium mb-2">Methodology</p>
          <p>Mode: {artifact.methodology.mode} | Phase model: {artifact.methodology.phase_model}</p>
          <p>Scenario: {artifact.methodology.scenario} | Sample phase: {artifact.methodology.sample_phase}</p>
          <p>Launches: {artifact.methodology.launch_count} | Phases: {artifact.methodology.phases_present?.join(', ')}</p>
        </div>
      )}
    </div>
  );
}

// ─── Sub-components ──────────────────────────────────────────────────────────

export function TimingRow({ row }: { row: TimingBreakdown }) {
  const successPct = row.totalCount > 0 ? (row.successCount / row.totalCount) * 100 : 0;
  return (
    <tr className="border-b border-gray-800/30 hover:bg-gray-800/10">
      <td className="px-4 py-2 text-gray-200 font-medium">{row.protocol}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{row.count}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{formatMs(row.avgDns)}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{formatMs(row.avgTcp)}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{formatMs(row.avgTls)}</td>
      <td className="px-4 py-2 text-gray-200 text-right">{formatMs(row.avgTtfb)}</td>
      <td className="px-4 py-2 text-gray-100 text-right font-bold">{formatMs(row.avgTotal)}</td>
      <td className={`px-4 py-2 text-right ${successRateClass(successPct)}`}>
        {row.successCount}/{row.totalCount}
      </td>
    </tr>
  );
}

export function StatsRow({ ps }: { ps: ProtocolStats }) {
  const fmt = (v: number) => formatMetricValue(ps.protocol, v);
  return (
    <tr className="border-b border-gray-800/30 hover:bg-gray-800/10">
      <td className="px-4 py-2 text-gray-200 font-medium">
        {ps.protocol}
        {ps.payloadBytes != null && <span className="text-gray-400 ml-1">({formatBytes(ps.payloadBytes)})</span>}
      </td>
      <td className="px-4 py-2 text-gray-400">{ps.label}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{ps.stats.count}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{fmt(ps.stats.min)}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{fmt(ps.stats.mean)}</td>
      <td className="px-4 py-2 text-gray-100 text-right font-semibold">{fmt(ps.stats.p50)}</td>
      <td className="px-4 py-2 text-yellow-400 text-right">{fmt(ps.stats.p95)}</td>
      <td className="px-4 py-2 text-s4 text-right">{fmt(ps.stats.p99)}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{fmt(ps.stats.max)}</td>
      <td className="px-4 py-2 text-gray-400 text-right">{fmt(ps.stats.stddev)}</td>
      <td className={`px-4 py-2 text-right ${successRateClass(ps.successRate)}`}>
        {ps.successRate.toFixed(0)}%
      </td>
    </tr>
  );
}

// Exported for RunDetailPage.attempts.test.tsx — renders one probe attempt's
// per-phase cards. Every row below the core timings is conditional: older runs
// (pre phase-detail widening) simply render fewer lines.
export function AttemptRow({ a }: { a: LiveAttempt }) {
  const st = a.server_timing;
  const hasSplit = st != null && (st.server_ms != null || st.network_ms != null || st.app_ms != null);
  const hasServerTimings = st != null && (hasSplit || st.processing_ms != null || st.total_server_ms != null);
  return (
    <div className="px-4 py-3 border-b border-gray-800/30 hover:bg-gray-800/10">
      <div className="flex items-center gap-4 mb-2">
        <span className="text-gray-400 text-xs w-8">#{a.sequence_num}</span>
        {a.success
          ? <span className="text-green-400 text-xs font-medium">OK</span>
          : <span className="text-red-400 text-xs font-medium">FAIL</span>
        }
        {a.retry_count > 0 && <span className="text-faint text-xs">{a.retry_count} retries</span>}
      </div>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-2 text-xs">
        {a.dns && (
          <SubResult label="DNS" color="gray">
            <p className="text-gray-300">{formatMs(a.dns.duration_ms)}</p>
            {a.dns.resolved_ips?.length > 0 && (
              <p className="text-gray-400 truncate">{a.dns.resolved_ips.join(', ')}</p>
            )}
          </SubResult>
        )}
        {a.tcp && (
          <SubResult label="TCP" color="gray">
            <p className="text-gray-300">{formatMs(a.tcp.connect_duration_ms)}</p>
            <p className="text-gray-400 truncate">{a.tcp.remote_addr}</p>
            {(a.tcp.total_retrans != null || a.tcp.congestion_algorithm != null || a.tcp.rtt_estimate_ms != null) && (
              <p className="truncate">
                {a.tcp.total_retrans != null && (
                  <span className={a.tcp.total_retrans > 0 ? 'text-yellow-400' : 'text-gray-400'}>
                    {a.tcp.total_retrans} retrans
                  </span>
                )}
                {a.tcp.congestion_algorithm != null && (
                  <span className="text-gray-400">{a.tcp.total_retrans != null ? ' · ' : ''}{a.tcp.congestion_algorithm}</span>
                )}
                {a.tcp.rtt_estimate_ms != null && (
                  <span className="text-gray-400"> · rtt {formatMs(a.tcp.rtt_estimate_ms)}</span>
                )}
              </p>
            )}
          </SubResult>
        )}
        {a.tls && (
          <SubResult label="TLS" color="gray">
            <p className="text-gray-300">{formatMs(a.tls.handshake_duration_ms)}</p>
            <p className="text-gray-400 truncate">{a.tls.protocol_version} · {a.tls.cipher_suite}</p>
            {(a.tls.handshake_kind != null || a.tls.resumed != null || a.tls.alpn_negotiated != null) && (
              <p className="truncate">
                {(a.tls.handshake_kind != null || a.tls.resumed != null) && (
                  <span className={(a.tls.resumed ?? a.tls.handshake_kind === 'resumed') ? 'text-cyan-400' : 'text-gray-400'}>
                    {a.tls.handshake_kind ?? (a.tls.resumed ? 'resumed' : 'full')}
                  </span>
                )}
                {a.tls.alpn_negotiated != null && (
                  <span className="text-gray-400">
                    {(a.tls.handshake_kind != null || a.tls.resumed != null) ? ' · ' : ''}alpn {a.tls.alpn_negotiated}
                  </span>
                )}
              </p>
            )}
          </SubResult>
        )}
        {a.http && (
          <SubResult label="HTTP" color="cyan">
            <p className="text-gray-300">
              <span className={a.http.status_code >= 400 ? 'text-red-400' : 'text-green-400'}>{a.http.status_code}</span>
              {' · '}TTFB {formatMs(a.http.ttfb_ms)} · Total {formatMs(a.http.total_duration_ms)}
            </p>
            <p className="text-gray-400 truncate">
              {a.http.negotiated_version}
              {a.http.throughput_mbps != null && ` · ${a.http.throughput_mbps.toFixed(1)} MB/s`}
              {a.http.goodput_mbps != null && ` · goodput ${a.http.goodput_mbps.toFixed(1)} MB/s`}
              {a.http.payload_bytes != null && a.http.payload_bytes > 0 && ` · ${formatBytes(a.http.payload_bytes)}`}
            </p>
          </SubResult>
        )}
        {a.udp && (
          <SubResult label="UDP" color="gray">
            <p className="text-gray-300">
              RTT avg {formatMs(a.udp.rtt_avg_ms)} · Loss {a.udp.loss_percent.toFixed(1)}%
              {a.udp.jitter_ms != null && ` · Jitter ${formatMs(a.udp.jitter_ms)}`}
            </p>
            <p className="text-gray-400">
              {a.udp.probe_count} probes
              {a.udp.rtt_p95_ms != null && ` · p95 ${formatMs(a.udp.rtt_p95_ms)}`}
            </p>
          </SubResult>
        )}
        {hasServerTimings && st && (
          <SubResult label="Server" color="cyan">
            {hasSplit ? (
              <>
                <p className="text-gray-300">
                  {st.server_ms != null && <>Server {formatMs(st.server_ms)}</>}
                  {st.server_ms != null && st.network_ms != null && ' · '}
                  {st.network_ms != null && <>Network {formatMs(st.network_ms)}</>}
                  {st.split_anomaly && (
                    <span className="text-yellow-400"> · split anomaly</span>
                  )}
                </p>
                {st.app_ms != null && <p className="text-gray-400">app {formatMs(st.app_ms)}</p>}
              </>
            ) : (
              <p className="text-gray-300">
                {st.processing_ms != null && <>Proc {formatMs(st.processing_ms)}</>}
                {st.processing_ms != null && st.total_server_ms != null && ' · '}
                {st.total_server_ms != null && <>Total {formatMs(st.total_server_ms)}</>}
              </p>
            )}
          </SubResult>
        )}
        {a.rpm && (
          <SubResult label="RPM" color="cyan">
            <p className="text-gray-300">
              RTT {formatMs(a.rpm.unloaded_rtt_avg_ms)} &rarr; {formatMs(a.rpm.loaded_rtt_avg_ms)} under load
              {a.rpm.rpm != null && ` · ${a.rpm.rpm.toFixed(0)} RPM`}
            </p>
            <p className="truncate">
              {a.rpm.bufferbloat_factor != null && (
                <span className={a.rpm.bufferbloat_factor >= 2 ? 'text-yellow-400' : 'text-gray-400'}>
                  bufferbloat &times;{a.rpm.bufferbloat_factor.toFixed(2)}
                </span>
              )}
              {a.rpm.load_throughput_mbps != null && (
                <span className="text-gray-400">
                  {a.rpm.bufferbloat_factor != null ? ' · ' : ''}load {a.rpm.load_throughput_mbps.toFixed(1)} MB/s
                </span>
              )}
            </p>
          </SubResult>
        )}
        {a.ping && (
          <SubResult label="Ping" color="gray">
            <p className="text-gray-300">
              RTT avg {formatMs(a.ping.rtt_avg_ms)} · Jitter {formatMs(a.ping.jitter_ms)} · Loss {a.ping.loss_percent.toFixed(1)}%
              {a.ping.fallback_method && (
                <span className="ml-2 rounded-sm border border-yellow-700 px-1.5 py-0.5 text-xs text-yellow-400" title={`Every ICMP echo was lost (cloud NAT layers drop ICMP), so the RTTs above are TCP connect times to port ${a.ping.fallback_port ?? '?'} — not comparable to ICMP RTTs.`}>
                  TCP RTT — ICMP blocked
                </span>
              )}
            </p>
            <p className="text-gray-400">
              {a.ping.probe_count} probes
              {a.ping.reply_ttl != null && ` · ttl ${a.ping.reply_ttl}`}
              {a.ping.fallback_method && a.ping.fallback_port != null && ` · tcp :${a.ping.fallback_port}`}
            </p>
          </SubResult>
        )}
        {a.path && (
          <SubResult label="Path" color="gray">
            <p className="text-gray-300">
              {a.path.hop_count != null ? `${a.path.hop_count} hops` : 'hops unknown'}
              {' · '}
              <span className={a.path.destination_reached ? 'text-green-400' : 'text-yellow-400'}>
                {a.path.destination_reached ? 'reached' : 'not reached'}
              </span>
              {a.path.destination_rtt_ms != null && ` · ${formatMs(a.path.destination_rtt_ms)}`}
            </p>
            <p className="text-gray-400 truncate">{a.path.method}</p>
            {a.path.hops.length > 0 && (
              <p className="text-gray-400 truncate">
                {a.path.hops.map((h) => h.addr ?? '*').join(' → ')}
              </p>
            )}
          </SubResult>
        )}
        {a.dualstack && (
          <SubResult label="Dual Stack" color="cyan">
            <p className="text-gray-300">
              v4 {a.dualstack.ipv4.success && a.dualstack.ipv4.total_ms != null
                ? formatMs(a.dualstack.ipv4.total_ms)
                : <span className={a.dualstack.ipv4.attempted ? 'text-red-400' : 'text-faint'}>{a.dualstack.ipv4.attempted ? 'fail' : 'n/a'}</span>}
              {' · '}
              v6 {a.dualstack.ipv6.success && a.dualstack.ipv6.total_ms != null
                ? formatMs(a.dualstack.ipv6.total_ms)
                : <span className={a.dualstack.ipv6.attempted ? 'text-red-400' : 'text-faint'}>{a.dualstack.ipv6.attempted ? 'fail' : 'n/a'}</span>}
              {a.dualstack.faster_family != null && (
                <span className="text-cyan-400">
                  {' · '}{a.dualstack.faster_family} faster
                  {a.dualstack.delta_ms != null && ` by ${formatMs(a.dualstack.delta_ms)}`}
                </span>
              )}
            </p>
            <p className="text-gray-400 truncate">{a.dualstack.happy_eyeballs_verdict}</p>
          </SubResult>
        )}
        {a.websocket && (
          <SubResult label="WebSocket" color="cyan">
            <p className="text-gray-300">
              Upgrade {formatMs(a.websocket.upgrade_ms)} · RTT avg {formatMs(a.websocket.msg_rtt_avg_ms)} · Loss {a.websocket.loss_percent.toFixed(1)}%
            </p>
            <p className="text-gray-400">
              {a.websocket.echo_count}/{a.websocket.message_count} echoes
              {` · p95 ${formatMs(a.websocket.msg_rtt_p95_ms)}`}
            </p>
          </SubResult>
        )}
        {a.pmtud && (
          <SubResult label="PMTUD" color="gray">
            <p className="text-gray-300">
              {a.pmtud.path_mtu != null
                ? <>Path MTU <span>{a.pmtud.path_mtu}</span>{a.pmtud.lower_bound_only && <span className="text-yellow-400"> (lower bound)</span>}</>
                : <span className="text-yellow-400">no MTU verdict</span>}
              {a.pmtud.local_mtu != null && ` · local ${a.pmtud.local_mtu}`}
            </p>
            <p className="text-gray-400 truncate">{a.pmtud.method}</p>
          </SubResult>
        )}
        {a.page_load && (
          <SubResult label="Page Load" color="blue">
            <p className="text-gray-300">Total {formatMs(a.page_load.total_ms)} · {a.page_load.assets_fetched}/{a.page_load.asset_count} assets</p>
            {a.page_load.tls_setup_ms != null && <p className="text-gray-400">TLS setup: {formatMs(a.page_load.tls_setup_ms)}</p>}
          </SubResult>
        )}
        {a.browser && (
          <SubResult label="Browser" color="purple">
            <p className="text-gray-300">Load {formatMs(a.browser.load_ms)}</p>
            {a.browser.dom_content_loaded_ms != null && <p className="text-gray-400">DCL: {formatMs(a.browser.dom_content_loaded_ms)}</p>}
          </SubResult>
        )}
        {a.error && (
          <SubResult label="Error" color="red">
            <p className="text-red-300">{a.error.category}: {a.error.message}</p>
            {a.error.detail && <p className="text-red-400/60 truncate">{a.error.detail}</p>}
          </SubResult>
        )}
      </div>
    </div>
  );
}

function SubResult({ label, color, children }: { label: string; color: string; children: ReactNode }) {
  const borderColor = color === 'red' ? 'border-red-500/20' : color === 'cyan' ? 'border-gray-600' : color === 'blue' ? 'border-blue-500/20' : color === 'purple' ? 'border-purple-500/20' : 'border-gray-800';
  const bgColor = color === 'red' ? 'bg-red-500/5' : 'bg-[var(--bg-base)]';
  const labelColor = color === 'red' ? 'text-red-400' : color === 'cyan' ? 'text-gray-300' : color === 'blue' ? 'text-blue-400' : color === 'purple' ? 'text-purple-400' : 'text-gray-400';
  return (
    <div className={`${bgColor} border ${borderColor} rounded p-2`}>
      <p className={`${labelColor} tracking-wider mb-1 text-xs font-medium`}>{label}</p>
      {children}
    </div>
  );
}
