using System.Text.Json;
using System.Text.Json.Nodes;
using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Pins the wire shape of <c>GET /api/projects/{id}/agents</c> (audit F2, P0):
/// the endpoint once drifted from the legacy Rust <c>{"agents":[...]}</c>
/// envelope to a bare array, which made the client's <c>res.agents</c>
/// undefined and black-screened the dashboard. These tests serialize the
/// typed response DTOs exactly as ASP.NET's <c>Results.Ok</c> does (web
/// defaults — irrelevant here because every property name is pinned with
/// <c>[JsonPropertyName]</c>) and assert the envelope, the per-item snake_case
/// field set, and that credentials can never appear.
/// </summary>
public sealed class AgentsContractTests
{
    private static readonly JsonSerializerOptions WebOptions =
        new(JsonSerializerDefaults.Web);

    private static AgentListItem SampleItem() => new(
        AgentId: Guid.Parse("6f9619ff-8b86-d011-b42d-00c04fc964ff"),
        Name: "agent-eastus-1",
        Region: "eastus",
        Provider: "azure",
        Status: "online",
        Version: "0.28.37",
        Os: "linux",
        Arch: "x86_64",
        LastHeartbeat: new DateTime(2026, 7, 14, 1, 22, 24, DateTimeKind.Utc),
        RegisteredAt: new DateTime(2026, 7, 1, 0, 0, 0, DateTimeKind.Utc),
        Tags: JsonNode.Parse("""{"pool":"default"}"""),
        TesterId: null);

    [Fact]
    public void List_response_is_an_object_with_a_top_level_agents_array()
    {
        var json = JsonSerializer.Serialize(
            new AgentListResponse(new[] { SampleItem() }), WebOptions);
        var root = JsonNode.Parse(json)!.AsObject();

        // The envelope IS the contract: `{ "agents": [...] }`, never a bare
        // array (the drift that caused the P0).
        Assert.Single(root);
        Assert.True(root.ContainsKey("agents"));
        Assert.IsType<JsonArray>(root["agents"]);
        Assert.Single(root["agents"]!.AsArray());
    }

    [Fact]
    public void Agent_item_emits_the_exact_snake_case_field_set()
    {
        var json = JsonSerializer.Serialize(SampleItem(), WebOptions);
        var item = JsonNode.Parse(json)!.AsObject();

        var expected = new[]
        {
            "agent_id", "name", "region", "provider", "status", "version",
            "os", "arch", "last_heartbeat", "registered_at", "tags", "tester_id",
        };

        Assert.Equal(expected, item.Select(p => p.Key).ToArray());
        Assert.Equal("6f9619ff-8b86-d011-b42d-00c04fc964ff", item["agent_id"]!.GetValue<string>());
        Assert.Equal("agent-eastus-1", item["name"]!.GetValue<string>());
        Assert.Equal("default", item["tags"]!["pool"]!.GetValue<string>());
    }

    [Fact]
    public void Agent_item_never_carries_credentials()
    {
        var json = JsonSerializer.Serialize(
            new AgentListResponse(new[] { SampleItem() }), WebOptions);

        Assert.DoesNotContain("api_key", json);
        Assert.DoesNotContain("apiKey", json);
    }
}
