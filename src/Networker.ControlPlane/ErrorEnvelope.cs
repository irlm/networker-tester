using Microsoft.AspNetCore.Diagnostics;

namespace Networker.ControlPlane;

/// <summary>
/// Global 500 contract. The 4xx surface is uniformly the homegrown
/// <c>{ "error": "..." }</c> envelope (each endpoint writes it explicitly);
/// before this middleware the 5xx surface was UNDEFINED — an unhandled
/// endpoint exception fell through to Kestrel's default empty 500. This pins
/// the 500 contract to the same envelope with a fixed, non-leaking message,
/// and logs the real exception server-side.
///
/// <para>Registered first in the pipeline (see <c>Program.cs</c>) so it wraps
/// auth, endpoints, and the raw-WS handlers alike. It only fires on unhandled
/// exceptions — every existing handled 4xx/5xx response is untouched.</para>
/// </summary>
public static class ErrorEnvelope
{
    /// <summary>The fixed 500 body — never carries exception details.</summary>
    public const string InternalErrorMessage = "internal server error";

    /// <summary>Install the exception-handler middleware emitting the uniform
    /// <c>{ "error": "internal server error" }</c> 500 envelope.</summary>
    public static IApplicationBuilder UseErrorEnvelope(this IApplicationBuilder app)
    {
        return app.UseExceptionHandler(errorApp => errorApp.Run(async context =>
        {
            var feature = context.Features.Get<IExceptionHandlerFeature>();

            // Server-side log with request context; the response stays opaque.
            context.RequestServices
                .GetRequiredService<ILoggerFactory>()
                .CreateLogger(typeof(ErrorEnvelope).FullName!)
                .LogError(
                    feature?.Error,
                    "Unhandled exception for {Method} {Path} — returning 500 envelope",
                    context.Request.Method, context.Request.Path);

            context.Response.StatusCode = StatusCodes.Status500InternalServerError;
            await context.Response.WriteAsJsonAsync(new { error = InternalErrorMessage });
        }));
    }
}
