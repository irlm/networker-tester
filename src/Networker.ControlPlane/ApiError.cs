namespace Networker.ControlPlane;

/// <summary>
/// The uniform 4xx error envelope — one place that spells
/// <c>{ "error": "..." }</c> instead of an inline anonymous object at every
/// endpoint return site. The wire shape is byte-identical to the previous
/// inline <c>new { error = message }</c> copies (same property name, same
/// serializer path through <see cref="Results"/>), and matches the 500 body
/// written by <see cref="ErrorEnvelope"/>.
/// </summary>
public static class ApiError
{
    /// <summary>400 with <c>{ "error": message }</c> — the shared replacement
    /// for <c>Results.BadRequest(new { error = ... })</c>.</summary>
    public static IResult BadRequest(string message) =>
        Results.BadRequest(new { error = message });

    /// <summary>404 with <c>{ "error": message }</c>.</summary>
    public static IResult NotFound(string message) =>
        Results.NotFound(new { error = message });

    /// <summary>409 with <c>{ "error": message }</c>.</summary>
    public static IResult Conflict(string message) =>
        Results.Conflict(new { error = message });

    /// <summary>Arbitrary status with <c>{ "error": message }</c> — for the
    /// non-canonical codes (401/403/423/429/409-via-Json, ...) that were
    /// previously spelled <c>Results.Json(new { error = ... }, statusCode: ...)</c>.
    /// </summary>
    public static IResult Status(int statusCode, string message) =>
        Results.Json(new { error = message }, statusCode: statusCode);
}
