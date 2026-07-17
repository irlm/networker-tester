using System.Text.Json;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// <see cref="ApiError"/> is the shared spelling of the uniform 4xx
/// <c>{ "error": "..." }</c> envelope that used to be ~90 inline
/// <c>new { error = ... }</c> copies across the endpoint modules. These tests
/// pin the wire contract the consolidation must preserve: status code,
/// <c>application/json</c>, and a body that is exactly one <c>error</c>
/// property carrying the message.
/// </summary>
public sealed class ApiErrorTests
{
    private static async Task<(int Status, string? ContentType, string Body)> ExecuteAsync(IResult result)
    {
        var ctx = new DefaultHttpContext
        {
            RequestServices = new ServiceCollection().AddLogging().BuildServiceProvider(),
        };
        ctx.Response.Body = new MemoryStream();

        await result.ExecuteAsync(ctx);

        ctx.Response.Body.Position = 0;
        using var reader = new StreamReader(ctx.Response.Body);
        return (ctx.Response.StatusCode, ctx.Response.ContentType, await reader.ReadToEndAsync());
    }

    private static void AssertEnvelope(string body, string expectedMessage)
    {
        using var doc = JsonDocument.Parse(body);
        Assert.Equal(JsonValueKind.Object, doc.RootElement.ValueKind);
        Assert.Equal(expectedMessage, doc.RootElement.GetProperty("error").GetString());
        // Exactly one property — the envelope carries nothing else.
        Assert.Single(doc.RootElement.EnumerateObject());
    }

    [Fact]
    public async Task BadRequest_is_400_with_the_error_envelope()
    {
        var (status, contentType, body) = await ExecuteAsync(ApiError.BadRequest("name is required"));

        Assert.Equal(StatusCodes.Status400BadRequest, status);
        Assert.StartsWith("application/json", contentType);
        AssertEnvelope(body, "name is required");
    }

    [Fact]
    public async Task NotFound_is_404_with_the_error_envelope()
    {
        var (status, _, body) = await ExecuteAsync(ApiError.NotFound("Tester not found"));

        Assert.Equal(StatusCodes.Status404NotFound, status);
        AssertEnvelope(body, "Tester not found");
    }

    [Fact]
    public async Task Conflict_is_409_with_the_error_envelope()
    {
        var (status, _, body) = await ExecuteAsync(ApiError.Conflict("Email already registered"));

        Assert.Equal(StatusCodes.Status409Conflict, status);
        AssertEnvelope(body, "Email already registered");
    }

    [Theory]
    [InlineData(StatusCodes.Status401Unauthorized)]
    [InlineData(StatusCodes.Status423Locked)]
    [InlineData(StatusCodes.Status429TooManyRequests)]
    [InlineData(StatusCodes.Status501NotImplemented)]
    public async Task Status_carries_arbitrary_codes_with_the_error_envelope(int code)
    {
        var (status, contentType, body) = await ExecuteAsync(ApiError.Status(code, "nope"));

        Assert.Equal(code, status);
        Assert.StartsWith("application/json", contentType);
        AssertEnvelope(body, "nope");
    }

    [Fact]
    public async Task Message_text_is_preserved_verbatim()
    {
        // Messages carry interpolated user data, unicode punctuation, quotes.
        const string message = "cloud_account 42 is a 'aws' account but the tester cloud is 'azure' — fix it";
        var (_, _, body) = await ExecuteAsync(ApiError.BadRequest(message));

        AssertEnvelope(body, message);
    }
}
