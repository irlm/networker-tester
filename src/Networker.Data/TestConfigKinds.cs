namespace Networker.Data;

/// <summary>
/// Stable product purposes for <c>test_config.test_kind</c>. A purpose says
/// which workflow owns a test; <c>endpoint_kind</c> independently says what it
/// targets.
/// </summary>
public static class TestConfigKinds
{
    public const string Network = "network";
    public const string UrlProbe = "url_probe";
    public const string SdkProbe = "sdk_probe";
    public const string Benchmark = "benchmark";

    public static bool IsValid(string value) => value is Network or UrlProbe or SdkProbe or Benchmark;
}
