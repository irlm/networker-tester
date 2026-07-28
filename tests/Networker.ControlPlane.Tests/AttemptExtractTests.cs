using System.Text.Json;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Unit tests for the pure attempt-JSON → <see cref="ParsedAttempt"/>
/// extraction that feeds V001 persistence (E2E P0-2). The SQL write is verified
/// against real Postgres by the live E2E rerun; these lock down the field
/// mapping (snake_case names, phase presence, IP join, type coercion).
/// </summary>
public class AttemptExtractTests
{
    private static JsonElement Json(string s) => JsonDocument.Parse(s).RootElement;

    private const string HttpAttempt = """
        {
          "attempt_id": "11111111-1111-1111-1111-111111111111",
          "protocol": "http2",
          "sequence_num": 3,
          "started_at": "2026-07-28T21:00:00Z",
          "finished_at": "2026-07-28T21:00:00.098Z",
          "success": true,
          "retry_count": 1,
          "dns": { "duration_ms": 8.5, "success": true, "query_name": "api.acme.com",
                   "resolved_ips": ["93.184.216.34", "2606:2800::1"] },
          "tcp": { "connect_duration_ms": 12.1, "remote_addr": "93.184.216.34:443",
                   "mss_bytes": 1460, "retransmits": 0, "snd_cwnd": 10, "min_rtt_ms": 11.9,
                   "congestion_algorithm": "cubic", "delivery_rate_bps": 1250000 },
          "tls": { "handshake_duration_ms": 22.4, "protocol_version": "TLSv1.3",
                   "cipher_suite": "TLS_AES_128_GCM_SHA256", "alpn_negotiated": "h2",
                   "cert_expiry": "2026-10-01T00:00:00Z" },
          "http": { "status_code": 200, "negotiated_version": "HTTP/2", "ttfb_ms": 40.2,
                    "total_duration_ms": 98.0, "body_size_bytes": 1256, "redirect_count": 0,
                    "payload_bytes": 65536, "throughput_mbps": 5.3 },
          "server_timing": { "processing_ms": 12.0, "recv_body_ms": 3.1, "total_server_ms": 15.1 }
        }
        """;

    [Fact]
    public void Parses_full_http_attempt_with_all_phases()
    {
        var runId = Guid.NewGuid();
        var a = AttemptExtract.Parse(runId, Json(HttpAttempt))!;

        Assert.Equal(Guid.Parse("11111111-1111-1111-1111-111111111111"), a.AttemptId);
        Assert.Equal(runId, a.RunId);
        Assert.Equal("http2", a.Protocol);
        Assert.Equal(3, a.SequenceNum);
        Assert.True(a.Success);
        Assert.Equal(1, a.RetryCount);
        Assert.NotNull(a.StartedAt);
        Assert.NotNull(a.FinishedAt);
        Assert.Contains("\"attempt_id\"", a.ExtraJson);   // full JSON retained

        Assert.Equal("api.acme.com", a.Dns!.QueryName);
        Assert.Equal("93.184.216.34,2606:2800::1", a.Dns.ResolvedIps);   // array joined
        Assert.Equal(8.5, a.Dns.DurationMs);

        Assert.Equal(1460, a.Tcp!.MssBytes);
        Assert.Equal(1_250_000, a.Tcp.DeliveryRateBps);   // long
        Assert.Equal("cubic", a.Tcp.CongestionAlgorithm);

        Assert.Equal("TLSv1.3", a.Tls!.ProtocolVersion);
        Assert.Equal("h2", a.Tls.AlpnNegotiated);
        Assert.Equal(new DateTime(2026, 10, 1, 0, 0, 0, DateTimeKind.Utc), a.Tls.CertExpiry!.Value);

        Assert.Equal(200, a.Http!.StatusCode);
        Assert.Equal(65536, a.Http.PayloadBytes);
        Assert.Equal(5.3, a.Http.ThroughputMbps);

        Assert.Equal(15.1, a.ServerTiming!.TotalServerMs);
        Assert.Null(a.Udp);   // absent phase stays null
    }

    [Fact]
    public void Parses_udp_only_attempt()
    {
        var a = AttemptExtract.Parse(Guid.NewGuid(), Json("""
            {
              "attempt_id": "22222222-2222-2222-2222-222222222222",
              "protocol": "udp", "sequence_num": 0, "success": true,
              "udp": { "rtt_avg_ms": 14.2, "rtt_min_ms": 12.0, "rtt_p95_ms": 20.1,
                       "jitter_ms": 1.4, "loss_percent": 0.5, "probe_count": 100, "success_count": 99 }
            }
            """))!;

        Assert.Equal("udp", a.Protocol);
        Assert.Null(a.Http);
        Assert.Equal(100, a.Udp!.ProbeCount);
        Assert.Equal(0.5, a.Udp.LossPercent);
        Assert.Equal(20.1, a.Udp.RttP95Ms);
    }

    [Fact]
    public void Failed_attempt_keeps_error_and_no_phases()
    {
        var a = AttemptExtract.Parse(Guid.NewGuid(), Json("""
            {
              "attempt_id": "33333333-3333-3333-3333-333333333333",
              "protocol": "http3", "sequence_num": 5, "success": false,
              "error_message": "connection timed out", "retry_count": 0
            }
            """))!;

        Assert.False(a.Success);
        Assert.Equal("connection timed out", a.ErrorMessage);
        Assert.Null(a.Http);
        Assert.Null(a.Dns);
    }

    [Fact]
    public void Missing_attempt_id_is_dropped_not_persisted_with_random_id()
    {
        Assert.Null(AttemptExtract.Parse(Guid.NewGuid(), Json("""{ "protocol": "http2" }""")));
        Assert.Null(AttemptExtract.Parse(Guid.NewGuid(), Json("""[]""")));
        Assert.Null(AttemptExtract.Parse(Guid.NewGuid(), Json("""123""")));
    }

    [Fact]
    public void Tolerates_wrong_kinds_and_defaults_sanely()
    {
        // sequence_num as string, success absent, dns wrong kind — must not throw.
        var a = AttemptExtract.Parse(Guid.NewGuid(), Json("""
            {
              "attempt_id": "44444444-4444-4444-4444-444444444444",
              "sequence_num": "not-a-number", "dns": "oops"
            }
            """))!;

        Assert.Equal("unknown", a.Protocol);   // protocol absent → default
        Assert.Equal(0, a.SequenceNum);        // unparseable → 0
        Assert.False(a.Success);               // absent → false
        Assert.Null(a.Dns);                    // wrong kind → null
    }
}
