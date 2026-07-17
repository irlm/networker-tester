using System.Text;
using System.Text.Json;
using Networker.ControlPlane.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// <see cref="CredentialJson"/> replaces the four byte-identical copies of the
/// decrypted-credential-JSON → string-map flattener (precheck, cloud accounts,
/// orphan reaper, tester create). These tests pin the shared behaviour:
/// string values pass through as-is, non-strings keep their raw JSON text,
/// non-object roots flatten to empty, and only the lenient variant swallows
/// malformed JSON.
/// </summary>
public sealed class CredentialJsonTests
{
    private static byte[] Utf8(string s) => Encoding.UTF8.GetBytes(s);

    [Fact]
    public void ToMap_flattens_string_values_as_is()
    {
        var map = CredentialJson.ToMap(Utf8(
            """{"subscription_id":"sub-1","client_secret":"s3cr3t","resource_group":""}"""));

        Assert.Equal(3, map.Count);
        Assert.Equal("sub-1", map["subscription_id"]);
        Assert.Equal("s3cr3t", map["client_secret"]);
        Assert.Equal(string.Empty, map["resource_group"]);
    }

    [Fact]
    public void ToMap_keeps_non_string_values_as_raw_json()
    {
        var map = CredentialJson.ToMap(Utf8(
            """{"port":5432,"enabled":true,"tags":["a","b"],"nested":{"k":"v"},"nothing":null}"""));

        Assert.Equal("5432", map["port"]);
        Assert.Equal("true", map["enabled"]);
        Assert.Equal("""["a","b"]""", map["tags"]);
        Assert.Equal("""{"k":"v"}""", map["nested"]);
        Assert.Equal("null", map["nothing"]);
    }

    [Theory]
    [InlineData("""["not","an","object"]""")]
    [InlineData("\"just a string\"")]
    [InlineData("42")]
    [InlineData("null")]
    public void ToMap_non_object_roots_flatten_to_empty(string json)
    {
        Assert.Empty(CredentialJson.ToMap(Utf8(json)));
    }

    [Fact]
    public void ToMap_invalid_json_throws_for_the_soft_fail_callers_to_catch()
    {
        // The precheck endpoint (decrypt_failed blocker) and the orphan reaper
        // (skip-account) rely on the throw to classify undecodable credentials.
        Assert.ThrowsAny<JsonException>(() => CredentialJson.ToMap(Utf8("not json {")));
    }

    [Fact]
    public void ToMapLenient_invalid_json_yields_empty()
    {
        Assert.Empty(CredentialJson.ToMapLenient("not json {"));
    }

    [Fact]
    public void ToMapLenient_parses_valid_objects_like_ToMap()
    {
        var map = CredentialJson.ToMapLenient("""{"region":"westeurope","zone":2}""");

        Assert.Equal(2, map.Count);
        Assert.Equal("westeurope", map["region"]);
        Assert.Equal("2", map["zone"]);
    }

    [Fact]
    public void Keys_are_case_sensitive_ordinal()
    {
        // Cloud credential keys are exact snake_case names; a differently-cased
        // key must NOT collide (ordinal comparer, matching every original copy).
        var map = CredentialJson.ToMap(Utf8("""{"Key":"upper","key":"lower"}"""));

        Assert.Equal(2, map.Count);
        Assert.Equal("upper", map["Key"]);
        Assert.Equal("lower", map["key"]);
    }
}
