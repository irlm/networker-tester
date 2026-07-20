namespace LagHound.Endpoint.Internal;

/// <summary>
/// The SDK package version reported on /health and /info (contract v1 §3.1).
/// A compile-time constant — no assembly reflection at request time (§6.6).
/// Keep in sync with the &lt;Version&gt; in LagHound.Endpoint.csproj.
/// </summary>
internal static class SdkVersion
{
    internal const string Value = "0.1.0";
}
