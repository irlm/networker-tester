using System.Security.Cryptography;
using System.Text;

namespace Networker.Monitoring;

public sealed class MonitoringApiKey(string value)
{
    public const string HeaderName = "X-LagHound-Monitoring-Key";
    private readonly byte[] _value = Encoding.UTF8.GetBytes(value);

    public bool Matches(string candidate)
    {
        var supplied = Encoding.UTF8.GetBytes(candidate);
        return supplied.Length == _value.Length
            && CryptographicOperations.FixedTimeEquals(supplied, _value);
    }
}

public sealed class MonitoringApiKeyMiddleware(RequestDelegate next)
{
    public async Task InvokeAsync(HttpContext context, MonitoringApiKey apiKey)
    {
        if (context.Request.Path.Equals("/health", StringComparison.OrdinalIgnoreCase))
        {
            await next(context);
            return;
        }

        var supplied = context.Request.Headers[MonitoringApiKey.HeaderName].FirstOrDefault();
        if (string.IsNullOrEmpty(supplied) || !apiKey.Matches(supplied))
        {
            context.Response.StatusCode = StatusCodes.Status401Unauthorized;
            await context.Response.WriteAsJsonAsync(new { error = "missing or invalid monitoring API key" });
            return;
        }

        await next(context);
    }
}
