using System.Text.Json;
using Networker.ControlPlane.Realtime;
using Networker.ControlPlane.Realtime.RawWs;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The runner tool inventory seam: heartbeat <c>capabilities</c> (additive
/// wire field, v0.28.208+) → <c>agent.tags.capabilities</c> (JSONB) →
/// <c>GET /api/projects/{id}/agents</c> / tester DTOs. Pins that an old agent's
/// heartbeat (no field) still decodes, that the merge preserves other tags and
/// is write-free when nothing changed, and that a pre-0.28.208 row reads as
/// "unknown" (null) rather than "no chrome".
/// </summary>
public class AgentCapabilityTagsTests
{
    [Fact]
    public void Heartbeat_without_capabilities_still_decodes_as_before()
    {
        var hb = Assert.IsType<HeartbeatMessage>(AgentMessageProcessor.Decode(
            """{"type":"heartbeat","load":0.1,"version":"0.28.207"}"""));
        Assert.Null(hb.Capabilities);
    }

    [Fact]
    public void Heartbeat_with_capabilities_decodes_the_inventory()
    {
        var hb = Assert.IsType<HeartbeatMessage>(AgentMessageProcessor.Decode(
            """{"type":"heartbeat","load":null,"version":"0.28.208","capabilities":{"chrome":true,"tshark":false}}"""));
        Assert.NotNull(hb.Capabilities);
        Assert.True(hb.Capabilities!.Chrome);
        Assert.False(hb.Capabilities.Tshark);
    }

    [Fact]
    public void Merge_writes_capabilities_under_the_key_and_preserves_other_tags()
    {
        var merged = AgentCapabilityTags.Merge("""{"env":"lab","zone":"eu"}""", new AgentCapabilities(true, false));
        using var doc = JsonDocument.Parse(merged);
        Assert.Equal("lab", doc.RootElement.GetProperty("env").GetString());
        Assert.Equal("eu", doc.RootElement.GetProperty("zone").GetString());
        var caps = doc.RootElement.GetProperty("capabilities");
        Assert.True(caps.GetProperty("chrome").GetBoolean());
        Assert.False(caps.GetProperty("tshark").GetBoolean());
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("not json")]
    [InlineData("[1,2]")]
    public void Merge_starts_a_fresh_object_when_tags_are_absent_or_not_an_object(string? tags)
    {
        var merged = AgentCapabilityTags.Merge(tags, new AgentCapabilities(false, true));
        var read = AgentCapabilityTags.Read(merged);
        Assert.NotNull(read);
        Assert.False(read!.Chrome);
        Assert.True(read.Tshark);
    }

    [Fact]
    public void Merge_is_a_no_op_when_the_stored_inventory_already_matches()
    {
        var stored = """{"capabilities":{"chrome":true,"tshark":true},"env":"lab"}""";
        var merged = AgentCapabilityTags.Merge(stored, new AgentCapabilities(true, true));
        Assert.Same(stored, merged);

        // …and rewrites when it differs.
        var changed = AgentCapabilityTags.Merge(stored, new AgentCapabilities(false, true));
        Assert.NotSame(stored, changed);
        Assert.False(AgentCapabilityTags.Read(changed)!.Chrome);
    }

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("""{"env":"lab"}""")]
    [InlineData("""{"capabilities":"yes"}""")]
    public void Read_returns_null_when_no_inventory_was_ever_reported(string? tags)
        => Assert.Null(AgentCapabilityTags.Read(tags));
}
