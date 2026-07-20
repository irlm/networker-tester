using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;

namespace LagHound.Endpoint.Internal;

/// <summary>
/// Immutable per-process runtime for the LagHound endpoint: validated config,
/// precomputed bodies, the shared download buffer, and the limiter instances.
/// Built once at mount time (contract v1 §3.1 precompute rule), then shared by
/// every request. No per-request allocation proportional to payload size.
/// </summary>
internal sealed class LagHoundRuntime
{
    /// <summary>SDK language tag reported on /health and /info (contract §3.1).</summary>
    internal const string SdkLang = "csharp";

    /// <summary>Fill byte for /download bodies: 0x42 = 'B' (contract §3.3, matches networker-endpoint DOWNLOAD_FILL).</summary>
    internal const byte DownloadFill = 0x42;

    /// <summary>Streaming chunk size for /download and /upload drain: 64 KiB (contract §3.3/§3.4).</summary>
    internal const int ChunkBytes = 65536;

    /// <summary>/echo request-body cap: 64 KiB, oversized → 413 (contract §6.1).</summary>
    internal const long EchoBodyMaxBytes = 65536;

    private readonly long _timestampAtInit;
    private readonly byte[] _tokenCurrent;
    private readonly byte[]? _tokenPrevious;

    internal string Prefix { get; }
    internal string? AppName { get; }
    internal long DownloadCapBytes { get; }
    internal long UploadCapBytes { get; }
    internal bool EnableEcho { get; }
    internal bool EnableDownload { get; }
    internal bool EnableUpload { get; }
    internal bool EnableInfo { get; }

    internal int RatePerIpRps { get; }
    internal int RatePerIpBurst { get; }
    internal int RateGlobalRps { get; }
    internal int RateGlobalBurst { get; }
    internal int MaxConcurrent { get; }
    internal int MaxConcurrentTransfers { get; }
    internal long? ByteBudgetBytes { get; }
    internal int ByteBudgetWindowSeconds { get; }

    internal PerIpRateLimiter PerIpLimiter { get; }
    internal TokenBucket GlobalLimiter { get; }
    internal ConcurrencyGate ConcurrencyGate { get; }
    internal ConcurrencyGate TransferGate { get; }
    internal ByteBudget? Budget { get; }

    /// <summary>Shared read-only source buffer for streaming /download (single per-process alloc).</summary>
    internal byte[] DownloadBuffer { get; }

    /// <summary>Precomputed /echo body: byte-for-byte constant for the process lifetime.</summary>
    internal byte[] EchoBody { get; }

    internal LagHoundRuntime(LagHoundOptions options)
    {
        ArgumentNullException.ThrowIfNull(options);

        string prefix = (options.Prefix ?? "/laghound").Trim();
        if (prefix.Length == 0 || prefix[0] != '/')
        {
            throw new ArgumentException("LagHound prefix must start with '/'.", nameof(options));
        }

        if (prefix.Length > 1 && prefix[^1] == '/')
        {
            throw new ArgumentException("LagHound prefix must not have a trailing slash.", nameof(options));
        }

        Prefix = prefix;

        // Token: programmatic first, then LAGHOUND_TOKEN env. Fail closed if neither (contract §2, §5).
        string? token = options.Token;
        if (string.IsNullOrEmpty(token))
        {
            token = Environment.GetEnvironmentVariable("LAGHOUND_TOKEN");
        }

        if (string.IsNullOrEmpty(token))
        {
            throw new InvalidOperationException(
                "LagHound refuses to mount without a token (fail-closed). Set LagHoundOptions.Token or the LAGHOUND_TOKEN environment variable.");
        }

        if (Encoding.UTF8.GetByteCount(token) < 16)
        {
            throw new InvalidOperationException("LagHound token must be at least 16 bytes (contract v1 §2).");
        }

        _tokenCurrent = Encoding.UTF8.GetBytes(token);
        _tokenPrevious = string.IsNullOrEmpty(options.PreviousToken)
            ? null
            : Encoding.UTF8.GetBytes(options.PreviousToken);

        AppName = string.IsNullOrWhiteSpace(options.AppName) ? null : options.AppName;

        // Clamp caps to the absolute maximum even if config asks for more (contract §2).
        DownloadCapBytes = Clamp(options.DownloadCapBytes, LagHoundOptions.AbsoluteMaxBytes);
        UploadCapBytes = Clamp(options.UploadCapBytes, LagHoundOptions.AbsoluteMaxBytes);

        EnableEcho = options.EnableEcho;
        EnableDownload = options.EnableDownload;
        EnableUpload = options.EnableUpload;
        EnableInfo = options.EnableInfo;

        RatePerIpRps = Math.Max(1, options.RatePerIpRps);
        RatePerIpBurst = Math.Max(1, options.RatePerIpBurst);
        RateGlobalRps = Math.Max(1, options.RateGlobalRps);
        RateGlobalBurst = Math.Max(1, options.RateGlobalBurst);
        MaxConcurrent = Math.Max(1, options.MaxConcurrent);
        MaxConcurrentTransfers = Math.Max(1, options.MaxConcurrentTransfers);

        ByteBudgetBytes = options.ByteBudgetBytes is > 0 ? options.ByteBudgetBytes : null;
        ByteBudgetWindowSeconds = Math.Max(1, options.ByteBudgetWindowSeconds);

        PerIpLimiter = new PerIpRateLimiter(RatePerIpRps, RatePerIpBurst, maxEntries: 10_000);
        GlobalLimiter = new TokenBucket(RateGlobalRps, RateGlobalBurst);
        ConcurrencyGate = new ConcurrencyGate(MaxConcurrent);
        TransferGate = new ConcurrencyGate(MaxConcurrentTransfers);
        Budget = ByteBudgetBytes is long b ? new ByteBudget(b, ByteBudgetWindowSeconds) : null;

        DownloadBuffer = new byte[ChunkBytes];
        Array.Fill(DownloadBuffer, DownloadFill);

        EchoBody = Encoding.UTF8.GetBytes("{\"contract\":\"v1\",\"ok\":true}");

        _timestampAtInit = Stopwatch.GetTimestamp();
    }

    internal long UptimeSeconds => (long)Stopwatch.GetElapsedTime(_timestampAtInit).TotalSeconds;

    /// <summary>
    /// Constant-time token comparison over the full length (contract §5). Uses
    /// <see cref="CryptographicOperations.FixedTimeEquals"/>; a length mismatch
    /// is folded into a fixed-length hash comparison so it does not short-circuit.
    /// </summary>
    internal bool TokenMatches(ReadOnlySpan<byte> presented)
    {
        bool ok = FixedTimeEqualsHashed(presented, _tokenCurrent);
        if (_tokenPrevious is not null)
        {
            // Always evaluate the second compare (no short-circuit on the first match).
            ok |= FixedTimeEqualsHashed(presented, _tokenPrevious);
        }

        return ok;
    }

    /// <summary>
    /// Length-safe constant-time compare: hash both sides to a fixed 32-byte
    /// digest first, so differing input lengths never produce an observable
    /// early-out (contract §5 "length mismatch MUST NOT short-circuit observably").
    /// </summary>
    private static bool FixedTimeEqualsHashed(ReadOnlySpan<byte> a, ReadOnlySpan<byte> b)
    {
        Span<byte> ha = stackalloc byte[32];
        Span<byte> hb = stackalloc byte[32];
        SHA256.HashData(a, ha);
        SHA256.HashData(b, hb);
        return CryptographicOperations.FixedTimeEquals(ha, hb);
    }

    private static long Clamp(long requested, long absoluteMax)
    {
        if (requested < 0)
        {
            return absoluteMax;
        }

        return Math.Min(requested, absoluteMax);
    }
}
