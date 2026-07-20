namespace LagHound.Endpoint;

/// <summary>
/// Configuration for the LagHound endpoint (contract v1 §2). Defaults mirror
/// <c>shared/sdk-contract-v1.json</c>; caps are clamped to the absolute
/// maximum at mount time even if config asks for more.
/// </summary>
public sealed class LagHoundOptions
{
    /// <summary>Absolute maximum transfer payload (32 MiB) — not configurable.</summary>
    public const long AbsoluteMaxBytes = 33_554_432;

    /// <summary>
    /// Shared secret. Min 16 bytes (UTF-8). Falls back to the
    /// <c>LAGHOUND_TOKEN</c> environment variable; if neither is set the SDK
    /// refuses to mount (fail-closed).
    /// </summary>
    public string? Token { get; set; }

    /// <summary>Optional previous token for zero-downtime rotation (contract §5, max 2 tokens).</summary>
    public string? PreviousToken { get; set; }

    /// <summary>Route prefix. Must start with '/', no trailing slash.</summary>
    public string Prefix { get; set; } = "/laghound";

    /// <summary>Optional label echoed on /health and /info. Never auto-derived from the host app.</summary>
    public string? AppName { get; set; }

    /// <summary>Effective cap for /download. Clamped to <see cref="AbsoluteMaxBytes"/>.</summary>
    public long DownloadCapBytes { get; set; } = 4_194_304;

    /// <summary>Effective cap for /upload. Clamped to <see cref="AbsoluteMaxBytes"/>.</summary>
    public long UploadCapBytes { get; set; } = 4_194_304;

    /// <summary>Per-IP token bucket refill rate (req/s).</summary>
    public int RatePerIpRps { get; set; } = 10;

    /// <summary>Per-IP token bucket burst capacity.</summary>
    public int RatePerIpBurst { get; set; } = 20;

    /// <summary>Global token bucket refill rate (req/s), across all LagHound routes.</summary>
    public int RateGlobalRps { get; set; } = 50;

    /// <summary>Global token bucket burst capacity.</summary>
    public int RateGlobalBurst { get; set; } = 100;

    /// <summary>Max in-flight LagHound requests per process.</summary>
    public int MaxConcurrent { get; set; } = 8;

    /// <summary>Max in-flight /download + /upload transfers combined.</summary>
    public int MaxConcurrentTransfers { get; set; } = 2;

    /// <summary>
    /// Optional transfer byte budget per window (contract §6.4). Off (null) by
    /// default. When exhausted, /download and /upload get 429 + Retry-After;
    /// /health, /echo and /info are never budget-limited.
    /// </summary>
    public long? ByteBudgetBytes { get; set; }

    /// <summary>Byte-budget window length in seconds.</summary>
    public int ByteBudgetWindowSeconds { get; set; } = 600;

    /// <summary>Mount /echo. Disabled routes are bare 404s and reported false in the /health capability map.</summary>
    public bool EnableEcho { get; set; } = true;

    /// <summary>Mount /download.</summary>
    public bool EnableDownload { get; set; } = true;

    /// <summary>Mount /upload.</summary>
    public bool EnableUpload { get; set; } = true;

    /// <summary>Mount /info.</summary>
    public bool EnableInfo { get; set; } = true;
}
