using System.Text.Json;
using System.Text.RegularExpressions;
using Networker.Contracts;
using Xunit;

namespace Networker.Tests;

/// <summary>
/// Guards the Rust↔C# JSON seam — the single most important contract in the
/// hybrid. A silent snake_case↔PascalCase mismatch would deserialize every
/// timing field to 0/null and the whole system would "work" while reporting
/// garbage. These tests fail loudly if a field stops round-tripping.
///
/// The golden fixture (fixtures/tester-golden.json) is REAL output captured
/// from <c>networker-tester --json-stdout</c> probing a local
/// <c>networker-endpoint</c> — never hand-typed. Regenerate it with
/// <c>scripts/regenerate-contract-golden.sh</c> whenever the tester's TestRun
/// schema changes, and commit the result alongside the schema change. Because
/// the payload is live capture, assertions are structural (fields present,
/// timings positive, version well-formed) rather than pinned to exact values.
/// </summary>
public class ContractRoundTripTests
{
    private static readonly string GoldenJson = File.ReadAllText(
        Path.Combine(AppContext.BaseDirectory, "fixtures", "tester-golden.json"));

    private static ProbeRunResult Deserialize(string json) =>
        JsonSerializer.Deserialize(json, ProbeContractJsonContext.Default.ProbeRunResult)!;

    [Fact]
    public void Golden_top_level_fields_round_trip()
    {
        var r = Deserialize(GoldenJson);

        Assert.Equal("1.0", r.SchemaVersion);
        Assert.True(Guid.TryParse(r.RunId, out _), $"run_id not a UUID: '{r.RunId}'");
        Assert.StartsWith("https://", r.TargetUrl);
        Assert.False(string.IsNullOrWhiteSpace(r.TargetHost));
        Assert.Contains("http1", r.Modes);
        Assert.NotEmpty(r.Attempts);

        // client_version is the tester's CARGO_PKG_VERSION — a dotted triple.
        // Not pinned to an exact value so the fixture doesn't have to be
        // regenerated on every release, only on schema changes.
        Assert.Matches(new Regex(@"^\d+\.\d+\.\d+$"), r.ClientVersion);
    }

    [Fact]
    public void Golden_per_phase_timings_round_trip()
    {
        var a = Deserialize(GoldenJson).Attempts[0];

        Assert.Equal("http1", a.Protocol);
        Assert.True(a.Success);

        // Non-null with positive durations proves the snake_case field names
        // still match — a naming mismatch would leave these 0/null.
        Assert.NotNull(a.Dns);
        Assert.True(a.Dns!.Success);
        Assert.True(a.Dns.DurationMs >= 0);

        Assert.NotNull(a.Tcp);
        Assert.True(a.Tcp!.Success);
        Assert.True(a.Tcp.ConnectDurationMs > 0, "tcp.connect_duration_ms not positive");

        Assert.NotNull(a.Tls);
        Assert.True(a.Tls!.Success);
        Assert.True(a.Tls.HandshakeDurationMs > 0, "tls.handshake_duration_ms not positive");
        Assert.False(string.IsNullOrWhiteSpace(a.Tls.ProtocolVersion));

        Assert.NotNull(a.Http);
        Assert.Equal(200, a.Http!.StatusCode);
        Assert.Equal("HTTP/1.1", a.Http.NegotiatedVersion);
        Assert.True(a.Http.TtfbMs > 0, "http.ttfb_ms not positive");
        Assert.True(a.Http.TotalDurationMs > 0, "http.total_duration_ms not positive");
    }

    [Fact]
    public void Golden_measurement_depth_fields_round_trip()
    {
        // The widened seam (measurement-gap-analysis-2026-07 finding #1) —
        // TLS negotiation depth, HTTP transfer detail, and server_timing must
        // come through from REAL tester output, not just the hand-written
        // full-fat sample below. Assertions stay structural (present /
        // well-formed) because the fixture is live capture: kernel TCP stats
        // and Server-Timing splits are platform/endpoint dependent, so only
        // the fields the golden capture is guaranteed to carry are pinned.
        var a = Deserialize(GoldenJson).Attempts[0];

        Assert.NotNull(a.Tls);
        Assert.False(string.IsNullOrWhiteSpace(a.Tls!.CipherSuite));
        Assert.False(string.IsNullOrWhiteSpace(a.Tls.AlpnNegotiated));
        Assert.False(string.IsNullOrWhiteSpace(a.Tls.TlsBackend));
        Assert.NotNull(a.Tls.CertExpiry);

        Assert.NotNull(a.Http);
        Assert.NotNull(a.Http!.BodySizeBytes);
        Assert.True(a.Http.BodySizeBytes > 0, "http.body_size_bytes not positive");
        Assert.NotNull(a.Http.RedirectCount);

        // server_timing is present on golden (networker-endpoint echoes
        // X-Networker-* headers) — must parse, even with only a subset of
        // fields populated.
        Assert.NotNull(a.ServerTiming);
    }

    [Fact]
    public void Full_fat_attempt_deserializes_every_widened_field()
    {
        // Hand-written payload exercising EVERY field the widened contract
        // models (a golden capture cannot guarantee kernel TCP stats or the
        // sdkprobe split on all platforms). Field names mirror
        // crates/networker-tester/src/metrics.rs exactly — a rename on either
        // side fails here.
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "0e0efd8e-6a94-41f5-a2c6-2c1f6c25d1cf",
          "attempts": [ {
            "attempt_id": "a1", "protocol": "download", "sequence_num": 4, "success": true,
            "dns": { "duration_ms": 5.2, "success": true },
            "tcp": {
              "connect_duration_ms": 1.5, "success": true,
              "mss_bytes": 1448, "rtt_estimate_ms": 12.25,
              "retransmits": 0, "total_retrans": 3, "snd_cwnd": 10,
              "congestion_algorithm": "bbr",
              "delivery_rate_bps": 1250000, "min_rtt_ms": 11.9
            },
            "tls": {
              "handshake_duration_ms": 9.1, "protocol_version": "TLSv1_3", "success": true,
              "cipher_suite": "TLS13_AES_256_GCM_SHA384", "alpn_negotiated": "h2",
              "cert_expiry": "2027-01-01T00:00:00Z",
              "resumed": true, "handshake_kind": "resumed", "tls_backend": "rustls"
            },
            "http": {
              "status_code": 200, "negotiated_version": "HTTP/2.0",
              "ttfb_ms": 20.5, "total_duration_ms": 180.0,
              "throughput_mbps": 41.7, "goodput_mbps": 39.2,
              "payload_bytes": 10485760, "body_size_bytes": 10485760,
              "redirect_count": 1, "cpu_time_ms": 6.4,
              "csw_voluntary": 42, "csw_involuntary": 7
            },
            "udp": {
              "rtt_avg_ms": 3.4, "rtt_min_ms": 2.1, "rtt_p95_ms": 6.7,
              "jitter_ms": 0.9, "loss_percent": 2.5,
              "probe_count": 40, "success_count": 39
            },
            "server_timing": {
              "server_ms": 8.5, "network_ms": 12.0, "app_ms": 8.5,
              "split_anomaly": true,
              "processing_ms": 7.9, "recv_body_ms": 1.2, "total_server_ms": 9.3
            }
          } ]
        }
        """;
        var a = Deserialize(json).Attempts[0];

        Assert.NotNull(a.Tcp);
        Assert.Equal(1448u, a.Tcp!.MssBytes);
        Assert.Equal(12.25, a.Tcp.RttEstimateMs);
        Assert.Equal(0u, a.Tcp.Retransmits);
        Assert.Equal(3u, a.Tcp.TotalRetrans);
        Assert.Equal(10u, a.Tcp.SndCwnd);
        Assert.Equal("bbr", a.Tcp.CongestionAlgorithm);
        Assert.Equal(1250000ul, a.Tcp.DeliveryRateBps);
        Assert.Equal(11.9, a.Tcp.MinRttMs);

        Assert.NotNull(a.Tls);
        Assert.Equal("TLS13_AES_256_GCM_SHA384", a.Tls!.CipherSuite);
        Assert.Equal("h2", a.Tls.AlpnNegotiated);
        Assert.Equal(new DateTimeOffset(2027, 1, 1, 0, 0, 0, TimeSpan.Zero), a.Tls.CertExpiry);
        Assert.True(a.Tls.Resumed);
        Assert.Equal("resumed", a.Tls.HandshakeKind);
        Assert.Equal("rustls", a.Tls.TlsBackend);

        Assert.NotNull(a.Http);
        Assert.Equal(41.7, a.Http!.ThroughputMbps);
        Assert.Equal(39.2, a.Http.GoodputMbps);
        Assert.Equal(10485760L, a.Http.PayloadBytes);
        Assert.Equal(10485760L, a.Http.BodySizeBytes);
        Assert.Equal(1u, a.Http.RedirectCount);
        Assert.Equal(6.4, a.Http.CpuTimeMs);
        Assert.Equal(42ul, a.Http.CswVoluntary);
        Assert.Equal(7ul, a.Http.CswInvoluntary);

        Assert.NotNull(a.Udp);
        Assert.Equal(3.4, a.Udp!.RttAvgMs);
        Assert.Equal(2.1, a.Udp.RttMinMs);
        Assert.Equal(6.7, a.Udp.RttP95Ms);
        Assert.Equal(0.9, a.Udp.JitterMs);
        Assert.Equal(2.5, a.Udp.LossPercent);
        Assert.Equal(40u, a.Udp.ProbeCount);
        Assert.Equal(39u, a.Udp.SuccessCount);

        Assert.NotNull(a.ServerTiming);
        Assert.Equal(8.5, a.ServerTiming!.ServerMs);
        Assert.Equal(12.0, a.ServerTiming.NetworkMs);
        Assert.Equal(8.5, a.ServerTiming.AppMs);
        Assert.True(a.ServerTiming.SplitAnomaly);
        Assert.Equal(7.9, a.ServerTiming.ProcessingMs);
        Assert.Equal(1.2, a.ServerTiming.RecvBodyMs);
        Assert.Equal(9.3, a.ServerTiming.TotalServerMs);
    }

    [Fact]
    public void Full_fat_measurement_depth_payload_deserializes_every_new_phase_and_envelope_field()
    {
        // v0.28.78 measurement-depth wave: six new attempt result types (rpm,
        // ping, path, dualstack, websocket, pmtud) and the new run-envelope
        // context (client_network, client_geo/target_geo, load samples,
        // clock_sync). Hand-written full-fat payload — field names mirror
        // crates/networker-tester/src/metrics.rs exactly; a rename on either
        // side fails here. Values follow Rust semantics (e.g. rpm = 60000 /
        // loaded avg RTT, bufferbloat_factor = loaded/unloaded avg).
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "0e0efd8e-6a94-41f5-a2c6-2c1f6c25d1cf",
          "client_network": {
            "default_interface": "en0", "interface_kind": "wifi", "mtu": 1500,
            "local_ip": "192.168.1.20", "gateway_ip": "192.168.1.1",
            "vpn_detected": true, "vpn_interface": "utun3", "ipv6_available": true
          },
          "client_geo": { "country": "SE", "city": "Linköping", "asn": 1257, "as_org": "Tele2 Sverige AB", "db_date": "2026-07-01" },
          "target_geo": { "country": "US", "asn": 13335, "as_org": "Cloudflare, Inc." },
          "client_load_before": { "load_avg_1m": 0.52, "mem_available_mb": 12288 },
          "client_load_after": { "load_avg_1m": 2.75, "mem_available_mb": 11020 },
          "clock_sync": { "ntp_server": "pool.ntp.org:123", "offset_ms": -12.5, "round_trip_ms": 34.2 },
          "attempts": [ {
            "attempt_id": "a1", "protocol": "rpm", "sequence_num": 0, "success": true,
            "rpm": {
              "remote_addr": "203.0.113.7:4000",
              "unloaded_probe_count": 20, "unloaded_success_count": 20,
              "unloaded_loss_percent": 0.0, "unloaded_rtt_min_ms": 8.1,
              "unloaded_rtt_avg_ms": 10.0, "unloaded_rtt_p95_ms": 14.2,
              "unloaded_jitter_ms": 0.8,
              "loaded_probe_count": 40, "loaded_success_count": 38,
              "loaded_loss_percent": 5.0, "loaded_rtt_min_ms": 12.3,
              "loaded_rtt_avg_ms": 85.0, "loaded_rtt_p95_ms": 190.4,
              "loaded_jitter_ms": 9.6,
              "rpm": 705.88, "bufferbloat_factor": 8.5,
              "load_duration_ms": 10000.0, "load_bytes_transferred": 524288000,
              "load_downloads_completed": 5, "load_throughput_mbps": 50.0,
              "started_at": "2026-07-20T12:00:00Z"
            },
            "ping": {
              "remote_addr": "203.0.113.7", "probe_count": 10, "success_count": 9,
              "loss_percent": 10.0, "rtt_min_ms": 7.9, "rtt_avg_ms": 9.4,
              "rtt_p95_ms": 12.6, "jitter_ms": 0.7,
              "probe_rtts_ms": [8.0, null, 9.1], "reply_ttl": 54,
              "started_at": "2026-07-20T12:00:01Z"
            },
            "path": {
              "remote_addr": "203.0.113.7:4000",
              "hops": [
                { "index": 1, "addr": "192.168.1.1", "rtt_ms": 1.2 },
                { "index": 2 },
                { "index": 3, "addr": "10.10.0.1", "rtt_ms": 6.5 }
              ],
              "hop_count": 3, "destination_reached": true,
              "destination_rtt_ms": 9.8, "method": "udp-ttl/ip-recverr",
              "max_ttl": 30, "started_at": "2026-07-20T12:00:02Z"
            },
            "dualstack": {
              "ipv4": {
                "attempted": true, "success": true, "addr": "203.0.113.7:443",
                "dns_ms": 4.0, "tcp_ms": 11.0, "tls_ms": 18.0,
                "ttfb_ms": 40.0, "total_ms": 55.0
              },
              "ipv6": {
                "attempted": true, "success": false, "addr": "[2001:db8::7]:443",
                "error": "connect timeout"
              },
              "faster_family": null, "delta_ms": null,
              "happy_eyeballs_verdict": "ipv4 (ipv6 connect failed)",
              "happy_eyeballs_grace_ms": 250.0,
              "started_at": "2026-07-20T12:00:03Z"
            },
            "websocket": {
              "url": "wss://example.com/ws", "upgrade_ms": 22.4, "upgrade_status": 101,
              "message_count": 20, "echo_count": 19, "loss_percent": 5.0,
              "msg_rtt_min_ms": 3.1, "msg_rtt_avg_ms": 4.6, "msg_rtt_p95_ms": 8.2,
              "jitter_ms": 0.5, "msg_rtts_ms": [3.5, null, 4.0],
              "payload_size": 125, "started_at": "2026-07-20T12:00:04Z"
            },
            "pmtud": {
              "remote_addr": "203.0.113.7:4000", "path_mtu": 1472,
              "max_unfragmented_payload": 1444, "probes_sent": 11,
              "method": "df-udp-echo/ip-recverr", "icmp_mtu": 1472,
              "local_mtu": 1500, "header_bytes": 28, "lower_bound_only": false,
              "started_at": "2026-07-20T12:00:05Z"
            }
          } ]
        }
        """;
        var r = Deserialize(json);

        // ── Run envelope ────────────────────────────────────────────────────
        Assert.NotNull(r.ClientNetwork);
        Assert.Equal("en0", r.ClientNetwork!.DefaultInterface);
        Assert.Equal("wifi", r.ClientNetwork.InterfaceKind);
        Assert.Equal(1500u, r.ClientNetwork.Mtu);
        Assert.Equal("192.168.1.20", r.ClientNetwork.LocalIp);
        Assert.Equal("192.168.1.1", r.ClientNetwork.GatewayIp);
        Assert.True(r.ClientNetwork.VpnDetected);
        Assert.Equal("utun3", r.ClientNetwork.VpnInterface);
        Assert.True(r.ClientNetwork.Ipv6Available);

        Assert.NotNull(r.ClientGeo);
        Assert.Equal("SE", r.ClientGeo!.Country);
        Assert.Equal("Linköping", r.ClientGeo.City);
        Assert.Equal(1257u, r.ClientGeo.Asn);
        Assert.Equal("Tele2 Sverige AB", r.ClientGeo.AsOrg);
        Assert.Equal("2026-07-01", r.ClientGeo.DbDate);

        Assert.NotNull(r.TargetGeo);
        Assert.Equal("US", r.TargetGeo!.Country);
        Assert.Null(r.TargetGeo.City); // absent field → null, never invented
        Assert.Equal(13335u, r.TargetGeo.Asn);

        Assert.NotNull(r.ClientLoadBefore);
        Assert.Equal(0.52, r.ClientLoadBefore!.LoadAvg1m);
        Assert.Null(r.ClientLoadBefore.CpuBusyPercent); // reserved, always null
        Assert.Equal(12288ul, r.ClientLoadBefore.MemAvailableMb);
        Assert.NotNull(r.ClientLoadAfter);
        Assert.Equal(2.75, r.ClientLoadAfter!.LoadAvg1m);

        Assert.NotNull(r.ClockSync);
        Assert.Equal("pool.ntp.org:123", r.ClockSync!.NtpServer);
        Assert.Equal(-12.5, r.ClockSync.OffsetMs);
        Assert.Equal(34.2, r.ClockSync.RoundTripMs);

        var a = r.Attempts[0];

        // ── rpm ─────────────────────────────────────────────────────────────
        Assert.NotNull(a.Rpm);
        Assert.Equal("203.0.113.7:4000", a.Rpm!.RemoteAddr);
        Assert.Equal(20u, a.Rpm.UnloadedProbeCount);
        Assert.Equal(20u, a.Rpm.UnloadedSuccessCount);
        Assert.Equal(0.0, a.Rpm.UnloadedLossPercent);
        Assert.Equal(8.1, a.Rpm.UnloadedRttMinMs);
        Assert.Equal(10.0, a.Rpm.UnloadedRttAvgMs);
        Assert.Equal(14.2, a.Rpm.UnloadedRttP95Ms);
        Assert.Equal(0.8, a.Rpm.UnloadedJitterMs);
        Assert.Equal(40u, a.Rpm.LoadedProbeCount);
        Assert.Equal(38u, a.Rpm.LoadedSuccessCount);
        Assert.Equal(5.0, a.Rpm.LoadedLossPercent);
        Assert.Equal(12.3, a.Rpm.LoadedRttMinMs);
        Assert.Equal(85.0, a.Rpm.LoadedRttAvgMs);
        Assert.Equal(190.4, a.Rpm.LoadedRttP95Ms);
        Assert.Equal(9.6, a.Rpm.LoadedJitterMs);
        Assert.Equal(705.88, a.Rpm.Rpm);
        Assert.Equal(8.5, a.Rpm.BufferbloatFactor);
        Assert.Equal(10000.0, a.Rpm.LoadDurationMs);
        Assert.Equal(524288000ul, a.Rpm.LoadBytesTransferred);
        Assert.Equal(5u, a.Rpm.LoadDownloadsCompleted);
        Assert.Equal(50.0, a.Rpm.LoadThroughputMbps);

        // ── ping ────────────────────────────────────────────────────────────
        Assert.NotNull(a.Ping);
        Assert.Equal("203.0.113.7", a.Ping!.RemoteAddr);
        Assert.Equal(10u, a.Ping.ProbeCount);
        Assert.Equal(9u, a.Ping.SuccessCount);
        Assert.Equal(10.0, a.Ping.LossPercent);
        Assert.Equal(7.9, a.Ping.RttMinMs);
        Assert.Equal(9.4, a.Ping.RttAvgMs);
        Assert.Equal(12.6, a.Ping.RttP95Ms);
        Assert.Equal(0.7, a.Ping.JitterMs);
        Assert.Equal(new double?[] { 8.0, null, 9.1 }, a.Ping.ProbeRttsMs);
        Assert.Equal(54u, a.Ping.ReplyTtl);

        // ── path ────────────────────────────────────────────────────────────
        Assert.NotNull(a.Path);
        Assert.Equal("203.0.113.7:4000", a.Path!.RemoteAddr);
        Assert.Equal(3, a.Path.Hops.Count);
        Assert.Equal(1u, a.Path.Hops[0].Index);
        Assert.Equal("192.168.1.1", a.Path.Hops[0].Addr);
        Assert.Equal(1.2, a.Path.Hops[0].RttMs);
        Assert.Null(a.Path.Hops[1].Addr); // silent hop (traceroute *)
        Assert.Null(a.Path.Hops[1].RttMs);
        Assert.Equal(3u, a.Path.HopCount);
        Assert.True(a.Path.DestinationReached);
        Assert.Equal(9.8, a.Path.DestinationRttMs);
        Assert.Equal("udp-ttl/ip-recverr", a.Path.Method);
        Assert.Equal(30u, a.Path.MaxTtl);

        // ── dualstack ───────────────────────────────────────────────────────
        Assert.NotNull(a.DualStack);
        Assert.NotNull(a.DualStack!.Ipv4);
        Assert.True(a.DualStack.Ipv4!.Attempted);
        Assert.True(a.DualStack.Ipv4.Success);
        Assert.Equal("203.0.113.7:443", a.DualStack.Ipv4.Addr);
        Assert.Equal(4.0, a.DualStack.Ipv4.DnsMs);
        Assert.Equal(11.0, a.DualStack.Ipv4.TcpMs);
        Assert.Equal(18.0, a.DualStack.Ipv4.TlsMs);
        Assert.Equal(40.0, a.DualStack.Ipv4.TtfbMs);
        Assert.Equal(55.0, a.DualStack.Ipv4.TotalMs);
        Assert.NotNull(a.DualStack.Ipv6);
        Assert.True(a.DualStack.Ipv6!.Attempted);
        Assert.False(a.DualStack.Ipv6.Success);
        Assert.Equal("connect timeout", a.DualStack.Ipv6.Error);
        Assert.Null(a.DualStack.Ipv6.TotalMs);
        Assert.Null(a.DualStack.FasterFamily); // only when BOTH legs succeed
        Assert.Null(a.DualStack.DeltaMs);
        Assert.Equal("ipv4 (ipv6 connect failed)", a.DualStack.HappyEyeballsVerdict);
        Assert.Equal(250.0, a.DualStack.HappyEyeballsGraceMs);

        // ── websocket ───────────────────────────────────────────────────────
        Assert.NotNull(a.WebSocket);
        Assert.Equal("wss://example.com/ws", a.WebSocket!.Url);
        Assert.Equal(22.4, a.WebSocket.UpgradeMs);
        Assert.Equal(101, a.WebSocket.UpgradeStatus);
        Assert.Equal(20u, a.WebSocket.MessageCount);
        Assert.Equal(19u, a.WebSocket.EchoCount);
        Assert.Equal(5.0, a.WebSocket.LossPercent);
        Assert.Equal(3.1, a.WebSocket.MsgRttMinMs);
        Assert.Equal(4.6, a.WebSocket.MsgRttAvgMs);
        Assert.Equal(8.2, a.WebSocket.MsgRttP95Ms);
        Assert.Equal(0.5, a.WebSocket.JitterMs);
        Assert.Equal(new double?[] { 3.5, null, 4.0 }, a.WebSocket.MsgRttsMs);
        Assert.Equal(125ul, a.WebSocket.PayloadSize);

        // ── pmtud ───────────────────────────────────────────────────────────
        Assert.NotNull(a.Pmtud);
        Assert.Equal("203.0.113.7:4000", a.Pmtud!.RemoteAddr);
        Assert.Equal(1472u, a.Pmtud.PathMtu);
        Assert.Equal(1444u, a.Pmtud.MaxUnfragmentedPayload);
        Assert.Equal(11u, a.Pmtud.ProbesSent);
        Assert.Equal("df-udp-echo/ip-recverr", a.Pmtud.Method);
        Assert.Equal(1472u, a.Pmtud.IcmpMtu);
        Assert.Equal(1500u, a.Pmtud.LocalMtu);
        Assert.Equal(28u, a.Pmtud.HeaderBytes);
        Assert.False(a.Pmtud.LowerBoundOnly);
    }

    [Fact]
    public void Old_minimal_payload_leaves_every_widened_field_null()
    {
        // Backward compatibility: a pre-widening payload (only the 4 phase
        // timings, none of the additive fields) must deserialize with all
        // widened fields null/default — never throw, never invent values.
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "r",
          "attempts": [ {
            "attempt_id": "a", "protocol": "http1", "sequence_num": 0, "success": true,
            "dns": { "duration_ms": 5.0, "success": true },
            "tcp": { "connect_duration_ms": 1.0, "success": true },
            "tls": { "handshake_duration_ms": 9.0, "protocol_version": "TLSv1_3", "success": true },
            "http": { "status_code": 200, "negotiated_version": "HTTP/1.1", "ttfb_ms": 2.0, "total_duration_ms": 3.0 }
          } ]
        }
        """;
        var r = Deserialize(json);
        var a = r.Attempts[0];

        // Measurement-depth run envelope (v0.28.78): absent on old payloads.
        Assert.Null(r.ClientNetwork);
        Assert.Null(r.ClientGeo);
        Assert.Null(r.TargetGeo);
        Assert.Null(r.ClientLoadBefore);
        Assert.Null(r.ClientLoadAfter);
        Assert.Null(r.ClockSync);

        Assert.NotNull(a.Tcp);
        Assert.Null(a.Tcp!.MssBytes);
        Assert.Null(a.Tcp.RttEstimateMs);
        Assert.Null(a.Tcp.Retransmits);
        Assert.Null(a.Tcp.TotalRetrans);
        Assert.Null(a.Tcp.SndCwnd);
        Assert.Null(a.Tcp.CongestionAlgorithm);
        Assert.Null(a.Tcp.DeliveryRateBps);
        Assert.Null(a.Tcp.MinRttMs);

        Assert.NotNull(a.Tls);
        Assert.Null(a.Tls!.CipherSuite);
        Assert.Null(a.Tls.AlpnNegotiated);
        Assert.Null(a.Tls.CertExpiry);
        Assert.Null(a.Tls.Resumed);
        Assert.Null(a.Tls.HandshakeKind);
        Assert.Null(a.Tls.TlsBackend);

        Assert.NotNull(a.Http);
        Assert.Null(a.Http!.ThroughputMbps);
        Assert.Null(a.Http.GoodputMbps);
        Assert.Null(a.Http.PayloadBytes);
        Assert.Null(a.Http.BodySizeBytes);
        Assert.Null(a.Http.RedirectCount);
        Assert.Null(a.Http.CpuTimeMs);
        Assert.Null(a.Http.CswVoluntary);
        Assert.Null(a.Http.CswInvoluntary);

        Assert.Null(a.Udp);
        Assert.Null(a.ServerTiming);

        // Measurement-depth attempt phases (v0.28.78): absent on old payloads.
        Assert.Null(a.Rpm);
        Assert.Null(a.Ping);
        Assert.Null(a.Path);
        Assert.Null(a.DualStack);
        Assert.Null(a.WebSocket);
        Assert.Null(a.Pmtud);
    }

    [Fact]
    public void Split_anomaly_defaults_false_when_absent()
    {
        // Rust skips serializing `split_anomaly: false` (skip_serializing_if =
        // Not::not) — absence MUST read back as false, not throw.
        const string json = """
        {
          "run_id": "r",
          "attempts": [ { "attempt_id": "a", "protocol": "sdkprobe", "sequence_num": 0, "success": true,
                          "server_timing": { "server_ms": 4.0, "network_ms": 6.0 } } ]
        }
        """;
        var st = Deserialize(json).Attempts[0].ServerTiming;

        Assert.NotNull(st);
        Assert.False(st!.SplitAnomaly);
        Assert.Equal(4.0, st.ServerMs);
        Assert.Equal(6.0, st.NetworkMs);
    }

    [Fact]
    public void Golden_unmodelled_rust_fields_are_ignored_not_thrown()
    {
        // The real payload carries many fields the C# layer does not model
        // (started_at, client_os, baseline, server_info, kernel TCP stats, …).
        // Deserialization must tolerate all of them — the whole point of the
        // versioned, additive seam. The raw JSON is checked to actually
        // contain such fields, so this test cannot silently weaken.
        using var doc = JsonDocument.Parse(GoldenJson);
        Assert.True(doc.RootElement.TryGetProperty("started_at", out _),
            "golden no longer carries unmodelled fields — regenerate it from the real tester");

        var ex = Record.Exception(() => Deserialize(GoldenJson));
        Assert.Null(ex);
    }

    [Fact]
    public void Missing_optional_phase_deserializes_to_null()
    {
        const string json = """
        {
          "schema_version": "1.0",
          "run_id": "r",
          "attempts": [ { "attempt_id": "a", "protocol": "dns", "sequence_num": 0, "success": true,
                          "dns": { "duration_ms": 5.0, "success": true } } ]
        }
        """;
        var a = Deserialize(json).Attempts[0];

        Assert.NotNull(a.Dns);
        Assert.Null(a.Tcp);
        Assert.Null(a.Tls);
        Assert.Null(a.Http);
    }

    [Fact]
    public void Missing_schema_version_does_not_crash()
    {
        // A pre-schema_version tester (or a partial payload) must still parse.
        // NOTE: System.Text.Json source-gen does NOT apply a C# property
        // initializer default (`= "unknown"`) to an ABSENT field — it leaves
        // it null. So consumers must not rely on the initializer for a missing
        // field. In practice the tester always emits schema_version (Phase 0),
        // so this only asserts resilience: parses without throwing.
        const string json = """{ "run_id": "r", "attempts": [] }""";
        var ex = Record.Exception(() => Deserialize(json));

        Assert.Null(ex);
        Assert.Empty(Deserialize(json).Attempts);
    }
}
