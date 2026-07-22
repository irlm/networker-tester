namespace Networker.ControlPlane.Security;

/// <summary>
/// Emits the baseline transport/response hardening headers on every control-plane
/// response (websec audit 2026-07, P1-3 — none were present at either the app or
/// nginx layer). The control plane serves JSON APIs + WebSocket upgrades, never
/// the SPA HTML (nginx serves the static bundle), so a deny-all CSP is safe here
/// and simply hardens any accidental HTML/error surface rather than constraining
/// a real page.
///
/// <para>Registered right after <c>UseErrorEnvelope</c> so the headers ride on
/// every response including the 500 envelope. HSTS is scoped to genuinely-HTTPS
/// requests (nginx terminates TLS and forwards <c>X-Forwarded-Proto: https</c>);
/// emitting it on the plain-HTTP health/agent traffic that also reaches Kestrel
/// would be ignored by browsers anyway. The canonical place for HSTS is still the
/// nginx TLS terminator — this is defence in depth.</para>
/// </summary>
public static class SecurityHeaders
{
    public static IApplicationBuilder UseSecurityHeaders(this IApplicationBuilder app)
    {
        return app.Use(async (context, next) =>
        {
            var h = context.Response.Headers;
            // Set (not append) so we are idempotent and a downstream handler can
            // still override if it ever needs to.
            h["X-Content-Type-Options"] = "nosniff";
            h["X-Frame-Options"] = "DENY";
            h["Referrer-Policy"] = "no-referrer";
            h["Content-Security-Policy"] = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";

            var forwardedProto = context.Request.Headers["X-Forwarded-Proto"].ToString();
            if (context.Request.IsHttps ||
                string.Equals(forwardedProto, "https", StringComparison.OrdinalIgnoreCase))
            {
                h["Strict-Transport-Security"] = "max-age=31536000; includeSubDomains";
            }

            await next();
        });
    }
}
