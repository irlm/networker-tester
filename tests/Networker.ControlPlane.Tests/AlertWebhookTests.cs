using System.Text.Json;
using Networker.ControlPlane.Alerting;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Webhook payload + HMAC signature construction — the contract documented in
/// docs/alerting.md that receivers verify against. The signature is pinned to
/// the RFC 4231 HMAC-SHA256 test vector so the implementation can never drift
/// to a different digest/encoding without this test failing.
/// </summary>
public sealed class AlertWebhookTests
{
    private static AlertNotification Sample() => new(
        event_id: Guid.Parse("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        rule_id: Guid.Parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        project_id: "usabc123def456",
        test_config_id: Guid.Parse("cccccccc-cccc-4ccc-8ccc-cccccccccccc"),
        run_id: Guid.Parse("dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
        metric: "p95_ms",
        comparator: "gt",
        threshold: 500,
        value: 812.5,
        state: "firing",
        message: "p95_ms 812.5 > 500 for 1 consecutive run(s)",
        fired_at: new DateTime(2026, 7, 18, 3, 0, 0, DateTimeKind.Utc));

    [Fact]
    public void Payload_is_snake_case_json_with_the_documented_fields()
    {
        var json = AlertWebhook.BuildPayloadJson(Sample());
        using var doc = JsonDocument.Parse(json);
        var root = doc.RootElement;

        Assert.Equal("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", root.GetProperty("event_id").GetString());
        Assert.Equal("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb", root.GetProperty("rule_id").GetString());
        Assert.Equal("usabc123def456", root.GetProperty("project_id").GetString());
        Assert.Equal("cccccccc-cccc-4ccc-8ccc-cccccccccccc", root.GetProperty("test_config_id").GetString());
        Assert.Equal("dddddddd-dddd-4ddd-8ddd-dddddddddddd", root.GetProperty("run_id").GetString());
        Assert.Equal("p95_ms", root.GetProperty("metric").GetString());
        Assert.Equal("gt", root.GetProperty("comparator").GetString());
        Assert.Equal(500.0, root.GetProperty("threshold").GetDouble());
        Assert.Equal(812.5, root.GetProperty("value").GetDouble());
        Assert.Equal("firing", root.GetProperty("state").GetString());
        Assert.Equal("p95_ms 812.5 > 500 for 1 consecutive run(s)", root.GetProperty("message").GetString());
        Assert.StartsWith("2026-07-18T03:00:00", root.GetProperty("fired_at").GetString());

        // Exactly the documented field set — additions must update the contract.
        Assert.Equal(12, root.EnumerateObject().Count());
    }

    [Fact]
    public void Project_wide_rules_serialize_a_null_test_config_id()
    {
        var n = Sample() with { test_config_id = null };
        using var doc = JsonDocument.Parse(AlertWebhook.BuildPayloadJson(n));
        Assert.Equal(JsonValueKind.Null, doc.RootElement.GetProperty("test_config_id").ValueKind);
    }

    [Fact]
    public void Payload_construction_is_deterministic()
    {
        // The signature covers the exact serialized bytes, so serialization
        // must be stable for identical input.
        Assert.Equal(AlertWebhook.BuildPayloadJson(Sample()), AlertWebhook.BuildPayloadJson(Sample()));
    }

    [Fact]
    public void Signature_matches_the_rfc4231_hmac_sha256_test_vector()
    {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
        var header = AlertWebhook.SignatureHeaderValue("what do ya want for nothing?", "Jefe");
        Assert.Equal(
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            header);
    }

    [Fact]
    public void Signature_header_name_is_stable()
    {
        // Documented in docs/alerting.md — renaming it breaks every receiver.
        Assert.Equal("X-Networker-Signature", AlertWebhook.SignatureHeader);
    }

    [Fact]
    public void Signature_changes_with_payload_and_secret()
    {
        var sig = AlertWebhook.SignatureHeaderValue("payload", "secret");
        Assert.NotEqual(sig, AlertWebhook.SignatureHeaderValue("payload2", "secret"));
        Assert.NotEqual(sig, AlertWebhook.SignatureHeaderValue("payload", "secret2"));
        Assert.StartsWith("sha256=", sig);
        Assert.Equal("sha256=".Length + 64, sig.Length); // 32 bytes, lowercase hex
    }

    // ── Channel-config validation (shared by the endpoints) ──────────────────

    [Fact]
    public void Webhook_config_requires_an_absolute_http_url()
    {
        var (_, err1) = AlertsEndpoints.ValidateChannelConfig(
            "webhook", JsonSerializer.SerializeToElement(new { url = "notaurl" }), null);
        Assert.NotNull(err1);

        var (_, err2) = AlertsEndpoints.ValidateChannelConfig(
            "webhook", JsonSerializer.SerializeToElement(new { url = "ftp://x/y" }), null);
        Assert.NotNull(err2);

        var (config, err3) = AlertsEndpoints.ValidateChannelConfig(
            "webhook", JsonSerializer.SerializeToElement(new { url = "https://hooks.example.com/x" }), null);
        Assert.Null(err3);
        Assert.Contains("https://hooks.example.com/x", config);
    }

    [Fact]
    public void Patching_back_the_masked_secret_preserves_the_stored_one()
    {
        var existing = JsonSerializer.Serialize(new { url = "https://h/x", secret = "real-secret" });

        var (config, err) = AlertsEndpoints.ValidateChannelConfig(
            "webhook",
            JsonSerializer.SerializeToElement(new { url = "https://h/x", secret = AlertsEndpoints.SecretMask }),
            existing);

        Assert.Null(err);
        using var doc = JsonDocument.Parse(config!);
        Assert.Equal("real-secret", doc.RootElement.GetProperty("secret").GetString());
    }

    [Fact]
    public void Email_config_requires_at_least_one_address()
    {
        var (_, err1) = AlertsEndpoints.ValidateChannelConfig(
            "email", JsonSerializer.SerializeToElement(new { to = Array.Empty<string>() }), null);
        Assert.NotNull(err1);

        var (_, err2) = AlertsEndpoints.ValidateChannelConfig(
            "email", JsonSerializer.SerializeToElement(new { to = new[] { "not-an-address" } }), null);
        Assert.NotNull(err2);

        var (config, err3) = AlertsEndpoints.ValidateChannelConfig(
            "email", JsonSerializer.SerializeToElement(new { to = new[] { "sre@example.com" } }), null);
        Assert.Null(err3);
        Assert.Contains("sre@example.com", config);
    }
}
