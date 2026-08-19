using System.Reflection;

namespace LagHound.Example;

internal static class DemoPage
{
    private const string ResourceName = "LagHound.Example.demo.html";

    public static string Html => Render();

    private static string Render()
    {
        using Stream? stream = Assembly.GetExecutingAssembly().GetManifestResourceStream(ResourceName);
        if (stream is null)
        {
            throw new InvalidOperationException($"Embedded demo page resource '{ResourceName}' was not found.");
        }

        using var reader = new StreamReader(stream);
        return reader.ReadToEnd()
            .Replace("{{LANGUAGE}}", "C#", StringComparison.Ordinal)
            .Replace("{{LANGUAGE_LABEL}}", "C# · ASP.NET CORE", StringComparison.Ordinal)
            .Replace("{{RUNTIME}}", "ASP.NET Core", StringComparison.Ordinal)
            .Replace("{{SOURCE_URL}}", "https://github.com/irlm/networker-tester/tree/main/sdk/csharp/Example", StringComparison.Ordinal)
            .Replace("{{MOUNT_SNIPPET}}", "builder.Services.AddLagHound(options =&gt; {\n  options.Token = configuration[\"LAGHOUND_TOKEN\"];\n});\napp.UseLagHound();", StringComparison.Ordinal);
    }
}
