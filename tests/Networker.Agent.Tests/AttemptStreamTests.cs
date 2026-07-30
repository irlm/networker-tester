using System.Text.Json;
using Networker.Agent;

namespace Networker.Agent.Tests;

/// <summary>
/// Streamed attempt-event parsing (NETWORKER_ATTEMPT_STREAM, tester >=
/// 0.28.117). The agent classifies tester stdout line-by-line: event lines
/// stream live as AttemptEventMessages; everything else buffers as the final
/// --json-stdout artifact. Misclassification either loses artifact data or
/// double-counts attempts, so the shapes are pinned here.
/// </summary>
public class AttemptStreamTests
{
    [Fact]
    public void Parses_a_real_event_line_to_the_inner_attempt()
    {
        // Shape emitted by dispatch.rs format_attempt_event (v0.28.117).
        var line = """{"event":"attempt","attempt":{"attempt_id":"859312d8-d1cd-4333-ba46-69c948b64e13","protocol":"dns","sequence_num":0,"success":true}}""";

        var attempt = RunExecutor.TryParseAttemptEvent(line);

        Assert.NotNull(attempt);
        Assert.Equal("dns", attempt.Value.GetProperty("protocol").GetString());
        Assert.True(attempt.Value.GetProperty("success").GetBoolean());
    }

    [Theory]
    [InlineData("""{"schema_version":"1.0","run_id":"x","attempts":[]}""")] // final artifact
    [InlineData("""{"event":"other","attempt":{}}""")]                      // wrong event
    [InlineData("""{"event":"attempt","attempt":"not-an-object"}""")]       // wrong shape
    [InlineData("""{"event":"attempt"}""")]                                 // missing attempt
    [InlineData("not json at all")]                                         // malformed
    [InlineData("")]                                                        // empty
    public void Non_event_lines_return_null_and_never_throw(string line)
        => Assert.Null(RunExecutor.TryParseAttemptEvent(line));

    [Fact]
    public void Parsed_element_survives_document_disposal()
    {
        // TryParseAttemptEvent Clone()s out of the parsed document — using the
        // element after return must not touch disposed memory.
        var attempt = RunExecutor.TryParseAttemptEvent(
            """{"event":"attempt","attempt":{"success":false,"protocol":"tcp"}}""");
        var json = attempt!.Value.GetRawText();
        Assert.Contains("\"tcp\"", json);
    }
}
