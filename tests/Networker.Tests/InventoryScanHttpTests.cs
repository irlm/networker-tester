using System.Net;
using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.Data.Entities;
using Networker.Security;

namespace Networker.Tests;

/// <summary>
/// End-to-end proof that "scan all providers" does something — the whole path
/// the owner clicks: real HTTP request → ProjectMember policy → real Postgres →
/// the project's encrypted cloud account decrypted → a real <c>az</c> process
/// spawned (a fake one, via <c>AZ_CMD</c>) → snake_case VM rows on the wire.
///
/// <para>Before this change the route was a stub that returned
/// <c>{vms: [], errors: []}</c> whatever the project had configured, which is
/// indistinguishable from a dead button — so an endpoint-level test that only
/// asserted "200 with well-formed JSON" (which the older suites do) would have
/// passed against the stub too. These assert the CONTENT.</para>
///
/// <para>The scan is read-only: the fake CLI refuses (exit 1) anything that is
/// not a sign-in or a list, and the test asserts no such call was made.</para>
///
/// <para>Its own fixture instance (own container + app), so the process-wide
/// <c>AZ_CMD</c> it sets cannot race another class's tests.</para>
/// </summary>
public sealed class InventoryScanHttpTests : IClassFixture<ControlPlaneFixture>, IDisposable
{
    private readonly ControlPlaneFixture _fixture;
    private readonly string _workDir;

    public InventoryScanHttpTests(ControlPlaneFixture fixture)
    {
        _fixture = fixture;
        _workDir = Path.Combine(Path.GetTempPath(), $"inventory-http-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_workDir);
    }

    public void Dispose()
    {
        Environment.SetEnvironmentVariable("AZ_CMD", null);
        try
        {
            Directory.Delete(_workDir, recursive: true);
        }
        catch
        {
            // best-effort temp cleanup
        }
    }

    private static string Url => $"/api/projects/{ControlPlaneFixture.SeededProjectId}/inventory";

    [Fact]
    public async Task Scan_without_cloud_accounts_says_which_providers_it_could_not_scan()
    {
        using var client = _fixture.CreateAuthenticatedClient();

        var resp = await client.GetAsync(Url);
        var body = JsonSerializer.Deserialize<JsonElement>(await resp.Content.ReadAsStringAsync());

        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Empty(body.GetProperty("vms").EnumerateArray());
        // Nothing failed — an unconfigured provider is an absence, not an error.
        Assert.Empty(body.GetProperty("errors").EnumerateArray());
        Assert.Empty(body.GetProperty("scanned").EnumerateArray());
        Assert.Equal(
            ["azure", "aws", "gcp"],
            body.GetProperty("not_configured").EnumerateArray().Select(e => e.GetString() ?? "").ToArray());
        // The UI stamps the empty state with this.
        Assert.True(body.GetProperty("scanned_at").TryGetDateTimeOffset(out _));
    }

    [Fact]
    public async Task Scan_is_denied_without_authentication()
    {
        using var client = _fixture.CreateClient();

        var resp = await client.GetAsync(Url);

        Assert.Equal(HttpStatusCode.Unauthorized, resp.StatusCode);
    }

    [Fact]
    public async Task Scan_enumerates_a_configured_azure_account_and_marks_managed_vms()
    {
        if (OperatingSystem.IsWindows())
        {
            return; // the fake CLI needs /bin/sh
        }

        var callLog = Path.Combine(_workDir, "az-calls.log");
        WriteFakeAz(callLog);

        var accountId = await SeedAzureAccountAsync();
        try
        {
            using var client = _fixture.CreateAuthenticatedClient();

            var resp = await client.GetAsync(Url);
            var raw = await resp.Content.ReadAsStringAsync();
            var body = JsonSerializer.Deserialize<JsonElement>(raw);

            Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
            Assert.Empty(body.GetProperty("errors").EnumerateArray());
            Assert.Equal(["azure"], body.GetProperty("scanned").EnumerateArray().Select(e => e.GetString() ?? "").ToArray());
            Assert.Equal(["aws", "gcp"],
                body.GetProperty("not_configured").EnumerateArray().Select(e => e.GetString() ?? "").ToArray());

            // The wire contract the dashboard's table reads, field by field.
            var vm = Assert.Single(body.GetProperty("vms").EnumerateArray());
            Assert.Equal("azure", vm.GetProperty("provider").GetString());
            Assert.Equal("nwk-ep-ubuntu-edne", vm.GetProperty("name").GetString());
            Assert.Equal("eastus", vm.GetProperty("region").GetString());
            Assert.Equal("running", vm.GetProperty("status").GetString());
            Assert.Equal("20.55.12.9", vm.GetProperty("public_ip").GetString());
            Assert.Equal(JsonValueKind.Null, vm.GetProperty("fqdn").ValueKind);
            Assert.Equal("Standard_B2s", vm.GetProperty("vm_size").GetString());
            Assert.Equal("linux", vm.GetProperty("os").GetString());
            Assert.Equal("networker-rg-endpoint", vm.GetProperty("resource_group").GetString());
            // Not cross-referenced to any deployment in this fixture.
            Assert.False(vm.GetProperty("managed").GetBoolean());

            // Read-only: a sign-in and a list, nothing else.
            var calls = File.Exists(callLog) ? File.ReadAllLines(callLog) : [];
            Assert.Equal(2, calls.Length);
            Assert.StartsWith("login --service-principal", calls[0], StringComparison.Ordinal);
            Assert.StartsWith("vm list --show-details", calls[1], StringComparison.Ordinal);
            Assert.DoesNotContain(calls, c => c.Contains("delete", StringComparison.Ordinal));
        }
        finally
        {
            await using var ctx = _fixture.NewDbContext();
            await ctx.CloudAccounts.Where(a => a.AccountId == accountId).ExecuteDeleteAsync();
        }
    }

    /// <summary>Insert an active Azure cloud account for the seeded project,
    /// encrypted with the running app's own cipher (so the endpoint decrypts it
    /// exactly as it would in production).</summary>
    private async Task<Guid> SeedAzureAccountAsync()
    {
        var cipher = _fixture.Services.GetRequiredService<CredentialCipher>();
        var (enc, nonce) = cipher.Encrypt(System.Text.Encoding.UTF8.GetBytes(JsonSerializer.Serialize(
            new Dictionary<string, string>
            {
                ["subscription_id"] = "sub-itest",
                ["client_id"] = "client-itest",
                ["client_secret"] = "secret-itest-long-enough",
                ["tenant_id"] = "tenant-itest",
            })));

        var accountId = Guid.NewGuid();
        await using var ctx = _fixture.NewDbContext();
        ctx.CloudAccounts.Add(new CloudAccount
        {
            AccountId = accountId,
            Name = "itest-azure",
            Provider = "azure",
            CredentialsEnc = enc,
            CredentialsNonce = nonce,
            Status = "active",
            ProjectId = ControlPlaneFixture.SeededProjectId,
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        await ctx.SaveChangesAsync();
        return accountId;
    }

    /// <summary>A fake <c>az</c> that logs each argv line, accepts the sign-in,
    /// answers <c>vm list</c> with one endpoint VM, and fails anything else.</summary>
    private void WriteFakeAz(string callLog)
    {
        const string ListJson =
            """[{"name":"nwk-ep-ubuntu-edne","rg":"networker-rg-endpoint","location":"eastus","powerState":"VM running","publicIps":"20.55.12.9","fqdns":"","size":"Standard_B2s","os":"Linux"}]""";

        var path = Path.Combine(_workDir, "fake-az.sh");
        File.WriteAllText(
            path,
            "#!/bin/sh\n" +
            $"printf '%s\\n' \"$*\" >> '{callLog}'\n" +
            "case \"$1\" in\n" +
            "  login) exit 0 ;;\n" +
            $"  vm) printf '%s' '{ListJson}'; exit 0 ;;\n" +
            "  *) printf 'unexpected az invocation\\n' >&2; exit 1 ;;\n" +
            "esac\n");
        if (!OperatingSystem.IsWindows())
        {
            // Guarded rather than suppressed: SetUnixFileMode throws on Windows,
            // where the caller has already returned early.
            File.SetUnixFileMode(
                path, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }

        Environment.SetEnvironmentVariable("AZ_CMD", path);
    }
}
