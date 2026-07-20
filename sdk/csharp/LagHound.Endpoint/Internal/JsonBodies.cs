using System.Buffers;
using System.Text.Json;

namespace LagHound.Endpoint.Internal;

/// <summary>
/// Hand-written JSON bodies via <see cref="Utf8JsonWriter"/> — no reflection,
/// no serializer (contract v1 §6.6). Health and info are rebuilt per request
/// only because <c>uptime_s</c> changes; every other field is constant.
/// </summary>
internal static class JsonBodies
{
    internal static byte[] Health(LagHoundRuntime rt)
    {
        var buffer = new ArrayBufferWriter<byte>(256);
        using var w = new Utf8JsonWriter(buffer);
        w.WriteStartObject();
        w.WriteString("contract", "v1");
        w.WriteString("status", "ok");
        w.WriteStartObject("sdk");
        w.WriteString("lang", LagHoundRuntime.SdkLang);
        w.WriteString("version", SdkVersion.Value);
        w.WriteEndObject();
        if (rt.AppName is not null)
        {
            w.WriteString("app", rt.AppName);
        }

        w.WriteNumber("uptime_s", rt.UptimeSeconds);
        WriteRoutes(w, rt);
        w.WriteEndObject();
        w.Flush();
        return buffer.WrittenSpan.ToArray();
    }

    internal static byte[] Info(LagHoundRuntime rt)
    {
        var buffer = new ArrayBufferWriter<byte>(512);
        using var w = new Utf8JsonWriter(buffer);
        w.WriteStartObject();
        w.WriteString("contract", "v1");
        w.WriteStartObject("sdk");
        w.WriteString("lang", LagHoundRuntime.SdkLang);
        w.WriteString("version", SdkVersion.Value);
        w.WriteEndObject();
        if (rt.AppName is not null)
        {
            w.WriteString("app", rt.AppName);
        }

        w.WriteString("prefix", rt.Prefix);
        w.WriteNumber("uptime_s", rt.UptimeSeconds);
        w.WriteBoolean("token_set", true);

        w.WriteStartObject("caps");
        w.WriteNumber("download_bytes", rt.DownloadCapBytes);
        w.WriteNumber("upload_bytes", rt.UploadCapBytes);
        w.WriteNumber("absolute_max_bytes", LagHoundOptions.AbsoluteMaxBytes);
        w.WriteEndObject();

        w.WriteStartObject("limits");
        w.WriteStartObject("rate_per_ip");
        w.WriteNumber("rps", rt.RatePerIpRps);
        w.WriteNumber("burst", rt.RatePerIpBurst);
        w.WriteEndObject();
        w.WriteStartObject("rate_global");
        w.WriteNumber("rps", rt.RateGlobalRps);
        w.WriteNumber("burst", rt.RateGlobalBurst);
        w.WriteEndObject();
        w.WriteNumber("max_concurrent", rt.MaxConcurrent);
        w.WriteNumber("max_concurrent_transfers", rt.MaxConcurrentTransfers);
        if (rt.ByteBudgetBytes is long budget)
        {
            w.WriteStartObject("byte_budget");
            w.WriteNumber("bytes", budget);
            w.WriteNumber("window_s", rt.ByteBudgetWindowSeconds);
            w.WriteEndObject();
        }
        else
        {
            w.WriteNull("byte_budget");
        }

        w.WriteEndObject();

        WriteRoutes(w, rt);
        w.WriteEndObject();
        w.Flush();
        return buffer.WrittenSpan.ToArray();
    }

    internal static byte[] UploadReceived(long receivedBytes)
    {
        var buffer = new ArrayBufferWriter<byte>(64);
        using var w = new Utf8JsonWriter(buffer);
        w.WriteStartObject();
        w.WriteString("contract", "v1");
        w.WriteNumber("received_bytes", receivedBytes);
        w.WriteEndObject();
        w.Flush();
        return buffer.WrittenSpan.ToArray();
    }

    /// <summary>Enveloped error body (contract §7). Messages are fixed strings — never interpolated request data.</summary>
    internal static byte[] Error(string code, string message, long? retryAfterMs)
    {
        var buffer = new ArrayBufferWriter<byte>(128);
        using var w = new Utf8JsonWriter(buffer);
        w.WriteStartObject();
        w.WriteString("contract", "v1");
        w.WriteStartObject("error");
        w.WriteString("code", code);
        w.WriteString("message", message);
        if (retryAfterMs is long ms)
        {
            w.WriteNumber("retry_after_ms", ms);
        }

        w.WriteEndObject();
        w.WriteEndObject();
        w.Flush();
        return buffer.WrittenSpan.ToArray();
    }

    private static void WriteRoutes(Utf8JsonWriter w, LagHoundRuntime rt)
    {
        w.WriteStartObject("routes");
        w.WriteBoolean("health", true);
        w.WriteBoolean("echo", rt.EnableEcho);
        w.WriteBoolean("download", rt.EnableDownload);
        w.WriteBoolean("upload", rt.EnableUpload);
        w.WriteBoolean("info", rt.EnableInfo);
        w.WriteEndObject();
    }
}
