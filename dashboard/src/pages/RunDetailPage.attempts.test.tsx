import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { AttemptRow } from './RunDetailPage';
import type { LiveAttempt } from '../api/types';

// Guards the run-detail probe cards over the widened attempt contract
// (measurement-gap-analysis-2026-07 finding #1): the high-value depth fields
// (network-vs-server split, TLS resumption, goodput, TCP retransmits/CC, UDP
// jitter) render when present — and old minimal attempts (pre-widening runs
// and flat REST rows) render without them and without crashing.

function attempt(overrides: Partial<LiveAttempt> = {}): LiveAttempt {
  return {
    attempt_id: 'a-1',
    run_id: 'r-1',
    protocol: 'http2',
    sequence_num: 7,
    started_at: '2026-07-20T12:00:00Z',
    finished_at: '2026-07-20T12:00:01Z',
    success: true,
    retry_count: 0,
    ...overrides,
  };
}

const fullFat: LiveAttempt = attempt({
  dns: { duration_ms: 5.2, query_name: 'example.com', resolved_ips: ['93.184.216.34'] },
  tcp: {
    connect_duration_ms: 1.5,
    remote_addr: '93.184.216.34:443',
    mss_bytes: 1448,
    rtt_estimate_ms: 12.25,
    retransmits: 0,
    total_retrans: 3,
    snd_cwnd: 10,
    congestion_algorithm: 'bbr',
    delivery_rate_bps: 1250000,
    min_rtt_ms: 11.9,
  },
  tls: {
    handshake_duration_ms: 9.1,
    protocol_version: 'TLSv1_3',
    cipher_suite: 'TLS13_AES_256_GCM_SHA384',
    alpn_negotiated: 'h2',
    cert_expiry: '2027-01-01T00:00:00Z',
    resumed: true,
    handshake_kind: 'resumed',
    tls_backend: 'rustls',
  },
  http: {
    status_code: 200,
    negotiated_version: 'HTTP/2.0',
    ttfb_ms: 20.5,
    total_duration_ms: 180.0,
    throughput_mbps: 41.7,
    goodput_mbps: 39.2,
    payload_bytes: 10485760,
    body_size_bytes: 10485760,
    redirect_count: 1,
    cpu_time_ms: 6.4,
    csw_voluntary: 42,
    csw_involuntary: 7,
  },
  udp: {
    rtt_avg_ms: 3.4,
    rtt_min_ms: 2.1,
    rtt_p95_ms: 6.7,
    jitter_ms: 0.9,
    loss_percent: 2.5,
    probe_count: 40,
    success_count: 39,
  },
  server_timing: {
    server_ms: 8.5,
    network_ms: 12.0,
    app_ms: 8.5,
    split_anomaly: true,
  },
});

describe('AttemptRow — widened phase detail', () => {
  it('renders the network-vs-server split with the anomaly flag', () => {
    render(<AttemptRow a={fullFat} />);

    expect(screen.getByText('Server')).toBeInTheDocument();
    expect(screen.getByText(/Server 8\.50ms/)).toBeInTheDocument();
    expect(screen.getByText(/Network 12\.00ms/)).toBeInTheDocument();
    expect(screen.getByText(/split anomaly/)).toBeInTheDocument();
    expect(screen.getByText(/app 8\.50ms/)).toBeInTheDocument();
  });

  it('renders TLS resumption, TCP kernel stats, goodput and UDP jitter', () => {
    render(<AttemptRow a={fullFat} />);

    // TLS: handshake kind + ALPN
    expect(screen.getByText(/resumed/)).toBeInTheDocument();
    expect(screen.getByText(/alpn h2/)).toBeInTheDocument();

    // TCP: lifetime retransmits + congestion algorithm + kernel RTT
    expect(screen.getByText(/3 retrans/)).toBeInTheDocument();
    expect(screen.getByText(/bbr/)).toBeInTheDocument();
    expect(screen.getByText(/rtt 12\.25ms/)).toBeInTheDocument();

    // HTTP: throughput AND goodput
    expect(screen.getByText(/41\.7 MB\/s/)).toBeInTheDocument();
    expect(screen.getByText(/goodput 39\.2 MB\/s/)).toBeInTheDocument();

    // UDP: jitter + p95 (formatMs renders sub-1ms values in µs)
    expect(screen.getByText(/Jitter 900µs/)).toBeInTheDocument();
    expect(screen.getByText(/p95 6\.70ms/)).toBeInTheDocument();
  });

  it('falls back to proc/total server timings when no split is present', () => {
    render(
      <AttemptRow
        a={attempt({ server_timing: { processing_ms: 7.9, total_server_ms: 9.3 } })}
      />
    );

    expect(screen.getByText('Server')).toBeInTheDocument();
    expect(screen.getByText(/Proc 7\.90ms/)).toBeInTheDocument();
    expect(screen.getByText(/Total 9\.30ms/)).toBeInTheDocument();
  });

  it('omits the server card when server_timing carries no usable timings', () => {
    render(<AttemptRow a={attempt({ server_timing: { clock_skew_ms: 0.2 } })} />);

    expect(screen.queryByText('Server')).not.toBeInTheDocument();
  });

  it('renders the rpm card with bufferbloat warn color when the factor is high', () => {
    render(
      <AttemptRow
        a={attempt({
          protocol: 'rpm',
          rpm: {
            remote_addr: '203.0.113.7:4000',
            unloaded_probe_count: 20,
            unloaded_success_count: 20,
            unloaded_loss_percent: 0,
            unloaded_rtt_min_ms: 8.1,
            unloaded_rtt_avg_ms: 10.0,
            unloaded_rtt_p95_ms: 14.2,
            unloaded_jitter_ms: 0.8,
            loaded_probe_count: 40,
            loaded_success_count: 38,
            loaded_loss_percent: 5.0,
            loaded_rtt_min_ms: 12.3,
            loaded_rtt_avg_ms: 85.0,
            loaded_rtt_p95_ms: 190.4,
            loaded_jitter_ms: 9.6,
            rpm: 705.88,
            bufferbloat_factor: 8.5,
            load_duration_ms: 10000,
            load_bytes_transferred: 524288000,
            load_downloads_completed: 5,
            load_throughput_mbps: 50.0,
          },
        })}
      />
    );

    expect(screen.getByText('RPM')).toBeInTheDocument();
    expect(screen.getByText(/10\.00ms/)).toBeInTheDocument(); // unloaded avg
    expect(screen.getByText(/85\.00ms under load/)).toBeInTheDocument();
    expect(screen.getByText(/706 RPM/)).toBeInTheDocument();
    const bloat = screen.getByText(/bufferbloat ×8\.50/);
    expect(bloat).toHaveClass('text-yellow-400'); // factor ≥ 2 → warn
    expect(screen.getByText(/load 50\.0 MB\/s/)).toBeInTheDocument();
  });

  it('renders bufferbloat factor without warn color when near 1.0', () => {
    render(
      <AttemptRow
        a={attempt({
          protocol: 'rpm',
          rpm: {
            remote_addr: '203.0.113.7:4000',
            unloaded_probe_count: 20,
            unloaded_success_count: 20,
            unloaded_loss_percent: 0,
            unloaded_rtt_min_ms: 8.1,
            unloaded_rtt_avg_ms: 10.0,
            unloaded_rtt_p95_ms: 14.2,
            unloaded_jitter_ms: 0.8,
            loaded_probe_count: 40,
            loaded_success_count: 40,
            loaded_loss_percent: 0,
            loaded_rtt_min_ms: 9.0,
            loaded_rtt_avg_ms: 11.0,
            loaded_rtt_p95_ms: 13.5,
            loaded_jitter_ms: 0.9,
            rpm: 5454.5,
            bufferbloat_factor: 1.1,
            load_duration_ms: 10000,
            load_bytes_transferred: 524288000,
            load_downloads_completed: 5,
          },
        })}
      />
    );

    expect(screen.getByText(/bufferbloat ×1\.10/)).toHaveClass('text-gray-500');
  });

  it('renders the dualstack card with per-family totals and the faster family', () => {
    render(
      <AttemptRow
        a={attempt({
          protocol: 'dualstack',
          dualstack: {
            ipv4: { attempted: true, success: true, addr: '203.0.113.7:443', total_ms: 55.0 },
            ipv6: { attempted: true, success: true, addr: '[2001:db8::7]:443', total_ms: 48.0 },
            faster_family: 'ipv6',
            delta_ms: 7.0,
            happy_eyeballs_verdict: 'ipv6 (connect within 250ms grace of ipv4)',
            happy_eyeballs_grace_ms: 250,
          },
        })}
      />
    );

    expect(screen.getByText('Dual Stack')).toBeInTheDocument();
    expect(screen.getByText(/v4 55\.00ms/)).toBeInTheDocument();
    expect(screen.getByText(/v6 48\.00ms/)).toBeInTheDocument();
    expect(screen.getByText(/ipv6 faster by 7\.00ms/)).toBeInTheDocument();
    expect(
      screen.getByText(/ipv6 \(connect within 250ms grace of ipv4\)/)
    ).toBeInTheDocument();
  });

  it('marks a failed dualstack leg instead of fabricating a total', () => {
    render(
      <AttemptRow
        a={attempt({
          protocol: 'dualstack',
          dualstack: {
            ipv4: { attempted: true, success: true, addr: '203.0.113.7:443', total_ms: 55.0 },
            ipv6: { attempted: true, success: false, error: 'connect timeout' },
            happy_eyeballs_verdict: 'ipv4 (ipv6 connect failed)',
            happy_eyeballs_grace_ms: 250,
          },
        })}
      />
    );

    expect(screen.getByText('fail')).toBeInTheDocument();
    expect(screen.queryByText(/faster/)).not.toBeInTheDocument();
  });

  it('renders the pmtud card with the MTU verdict and method', () => {
    render(
      <AttemptRow
        a={attempt({
          protocol: 'pmtud',
          pmtud: {
            remote_addr: '203.0.113.7:4000',
            path_mtu: 1472,
            max_unfragmented_payload: 1444,
            probes_sent: 11,
            method: 'df-udp-echo/ip-recverr',
            icmp_mtu: 1472,
            local_mtu: 1500,
            header_bytes: 28,
            lower_bound_only: false,
          },
        })}
      />
    );

    expect(screen.getByText('PMTUD')).toBeInTheDocument();
    expect(screen.getByText('1472')).toBeInTheDocument();
    expect(screen.getByText(/local 1500/)).toBeInTheDocument();
    expect(screen.getByText('df-udp-echo/ip-recverr')).toBeInTheDocument();
    expect(screen.queryByText(/no MTU verdict/)).not.toBeInTheDocument();
    expect(screen.queryByText(/lower bound/)).not.toBeInTheDocument();
  });

  it('renders the pmtud no-feedback case honestly (no fabricated MTU)', () => {
    render(
      <AttemptRow
        a={attempt({
          protocol: 'pmtud',
          pmtud: {
            remote_addr: '203.0.113.7:4000',
            probes_sent: 4,
            method: 'df-no-feedback',
            header_bytes: 28,
            lower_bound_only: false,
          },
        })}
      />
    );

    expect(screen.getByText(/no MTU verdict/)).toBeInTheDocument();
    expect(screen.getByText('df-no-feedback')).toBeInTheDocument();
  });

  it('renders ping, path and websocket cards when present', () => {
    render(
      <AttemptRow
        a={attempt({
          ping: {
            remote_addr: '203.0.113.7',
            probe_count: 10,
            success_count: 9,
            loss_percent: 10.0,
            rtt_min_ms: 7.9,
            rtt_avg_ms: 9.4,
            rtt_p95_ms: 12.6,
            jitter_ms: 0.7,
            probe_rtts_ms: [8.0, null, 9.1],
            reply_ttl: 54,
          },
          path: {
            remote_addr: '203.0.113.7:4000',
            hops: [
              { index: 1, addr: '192.168.1.1', rtt_ms: 1.2 },
              { index: 2 },
              { index: 3, addr: '10.10.0.1', rtt_ms: 6.5 },
            ],
            hop_count: 3,
            destination_reached: true,
            destination_rtt_ms: 9.8,
            method: 'udp-ttl/ip-recverr',
            max_ttl: 30,
          },
          websocket: {
            url: 'wss://example.com/ws',
            upgrade_ms: 22.4,
            upgrade_status: 101,
            message_count: 20,
            echo_count: 19,
            loss_percent: 5.0,
            msg_rtt_min_ms: 3.1,
            msg_rtt_avg_ms: 4.6,
            msg_rtt_p95_ms: 8.2,
            jitter_ms: 0.5,
            msg_rtts_ms: [3.5, null, 4.0],
            payload_size: 125,
          },
        })}
      />
    );

    // Ping
    expect(screen.getByText('Ping')).toBeInTheDocument();
    expect(screen.getByText(/RTT avg 9\.40ms · Jitter 700µs · Loss 10\.0%/)).toBeInTheDocument();
    expect(screen.getByText(/10 probes · ttl 54/)).toBeInTheDocument();

    // Path — silent hop rendered as a traceroute '*'
    expect(screen.getByText('Path')).toBeInTheDocument();
    expect(screen.getByText(/3 hops/)).toBeInTheDocument();
    expect(screen.getByText('reached')).toBeInTheDocument();
    expect(screen.getByText('192.168.1.1 → * → 10.10.0.1')).toBeInTheDocument();

    // WebSocket
    expect(screen.getByText('WebSocket')).toBeInTheDocument();
    expect(screen.getByText(/Upgrade 22\.40ms · RTT avg 4\.60ms · Loss 5\.0%/)).toBeInTheDocument();
    expect(screen.getByText(/19\/20 echoes · p95 8\.20ms/)).toBeInTheDocument();
  });

  it('renders an old minimal attempt without any of the widened rows', () => {
    render(
      <AttemptRow
        a={attempt({
          tcp: { connect_duration_ms: 1.5, remote_addr: '10.0.0.1:443' },
          tls: {
            handshake_duration_ms: 9.1,
            protocol_version: 'TLSv1_3',
            cipher_suite: 'TLS13_AES_256_GCM_SHA384',
          },
          http: {
            status_code: 200,
            negotiated_version: 'HTTP/1.1',
            ttfb_ms: 2.0,
            total_duration_ms: 3.0,
          },
        })}
      />
    );

    expect(screen.getByText('#7')).toBeInTheDocument();
    expect(screen.getByText('OK')).toBeInTheDocument();
    expect(screen.queryByText('Server')).not.toBeInTheDocument();
    expect(screen.queryByText(/retrans/)).not.toBeInTheDocument();
    expect(screen.queryByText(/goodput/)).not.toBeInTheDocument();
    expect(screen.queryByText(/alpn/)).not.toBeInTheDocument();
    // Measurement-depth cards (v0.28.78) are strictly data-gated
    expect(screen.queryByText('RPM')).not.toBeInTheDocument();
    expect(screen.queryByText('Ping')).not.toBeInTheDocument();
    expect(screen.queryByText('Path')).not.toBeInTheDocument();
    expect(screen.queryByText('Dual Stack')).not.toBeInTheDocument();
    expect(screen.queryByText('WebSocket')).not.toBeInTheDocument();
    expect(screen.queryByText('PMTUD')).not.toBeInTheDocument();
  });
});
