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
  });
});
