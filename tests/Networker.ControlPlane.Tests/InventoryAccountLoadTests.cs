using System.Text;
using System.Text.Json;
using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Networker.ControlPlane.Endpoints;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Which of a project's cloud accounts the inventory scan actually uses
/// (<c>InventoryEndpoints.LoadAccountsAsync</c>), against a real Sqlite-backed
/// <see cref="NetworkerDbContext"/>.
///
/// <para>The design fork this pins: an account that <b>exists</b> but cannot be
/// scanned (pending/errored, undecryptable, no credential key on the host) is an
/// <c>errors[]</c> line naming it — while a provider with <b>no</b> account is
/// not an error at all, only an absence the caller reports as
/// <c>not_configured</c>. Silence for either one is what made the button look
/// dead.</para>
/// </summary>
public sealed class InventoryAccountLoadTests
{
    private const string ProjectId = "proj-000000001";

    // A fixed 32-byte AES key so encrypt/decrypt round-trips in-test.
    private static CredentialCipher NewCipher() =>
        new(Enumerable.Range(0, 32).Select(i => (byte)i).ToArray());

    private static (NetworkerDbContext Db, SqliteConnection Conn) NewDb()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();

        // Only the table the account load queries — with the real column names.
        // (The full Postgres model can't be built on Sqlite.)
        using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = """
                CREATE TABLE cloud_account (
                    account_id TEXT PRIMARY KEY,
                    owner_id TEXT,
                    name TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    credentials_enc BLOB NOT NULL,
                    credentials_nonce BLOB NOT NULL,
                    region_default TEXT,
                    status TEXT NOT NULL,
                    last_validated TEXT,
                    validation_error TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    project_id TEXT NOT NULL
                );
                """;
            cmd.ExecuteNonQuery();
        }

        return (sp.GetRequiredService<NetworkerDbContext>(), conn);
    }

    private static void AddAccount(
        NetworkerDbContext db, CredentialCipher cipher, string provider, string name,
        Dictionary<string, string> credentials, string status = "active",
        string? regionDefault = null, string projectId = ProjectId, bool corruptCiphertext = false)
    {
        var (enc, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(credentials)));
        if (corruptCiphertext)
        {
            enc[0] ^= 0xFF; // fails the GCM tag check, like a rotated key would
        }

        db.CloudAccounts.Add(new Data.Entities.CloudAccount
        {
            AccountId = Guid.NewGuid(),
            Name = name,
            Provider = provider,
            CredentialsEnc = enc,
            CredentialsNonce = nonce,
            RegionDefault = regionDefault,
            Status = status,
            ProjectId = projectId,
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
    }

    private static Task<(List<Provisioning.InventoryAccount> Accounts, List<string> Errors, HashSet<string> Configured)>
        LoadAsync(NetworkerDbContext db, CredentialCipher? cipher) =>
        InventoryEndpoints.LoadAccountsAsync(db, cipher, ProjectId, NullLogger.Instance, default);

    [Fact]
    public async Task Active_accounts_are_decrypted_and_carry_their_region_default()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAccount(db, cipher, "aws", "aws-prod",
            new Dictionary<string, string> { ["access_key_id"] = "AKIA1", ["secret_access_key"] = "shh" },
            regionDefault: "sa-east-1");

        var (accounts, errors, configured) = await LoadAsync(db, cipher);

        var account = Assert.Single(accounts);
        Assert.Equal("aws", account.Provider);
        Assert.Equal("aws-prod", account.Name);
        Assert.Equal("AKIA1", account.Credentials["access_key_id"]);
        Assert.Equal("sa-east-1", account.RegionDefault);
        Assert.Empty(errors);
        Assert.Equal(["aws"], configured);
    }

    [Fact]
    public async Task Another_projects_accounts_are_never_scanned()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAccount(db, cipher, "gcp", "someone-elses",
            new Dictionary<string, string> { ["json_key"] = "{}" }, projectId: "proj-000000002");

        var (accounts, errors, configured) = await LoadAsync(db, cipher);

        Assert.Empty(accounts);
        Assert.Empty(errors);
        Assert.Empty(configured);  // → the endpoint reports gcp as not_configured
    }

    [Theory]
    [InlineData("pending")]
    [InlineData("error")]
    public async Task A_non_active_account_is_named_in_errors_not_silently_skipped(string status)
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAccount(db, cipher, "azure", "az-broken",
            new Dictionary<string, string> { ["subscription_id"] = "sub-1" }, status: status);

        var (accounts, errors, configured) = await LoadAsync(db, cipher);

        Assert.Empty(accounts);
        var error = Assert.Single(errors);
        Assert.Contains("az-broken", error, StringComparison.Ordinal);
        Assert.Contains($"'{status}', not active", error, StringComparison.Ordinal);
        // Configured (so it is NOT reported as "no account for azure") but not scanned.
        Assert.Equal(["azure"], configured);
    }

    [Fact]
    public async Task An_undecryptable_account_is_reported_and_never_stops_the_others()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAccount(db, cipher, "azure", "az-rotated",
            new Dictionary<string, string> { ["subscription_id"] = "sub-1" }, corruptCiphertext: true);
        AddAccount(db, cipher, "gcp", "gcp-fine",
            new Dictionary<string, string> { ["json_key"] = "{}" });

        var (accounts, errors, configured) = await LoadAsync(db, cipher);

        Assert.Equal("gcp-fine", Assert.Single(accounts).Name);
        Assert.Contains("failed to decrypt", Assert.Single(errors), StringComparison.Ordinal);
        Assert.Equal(2, configured.Count);
    }

    [Fact]
    public async Task Without_a_credential_key_the_host_says_so_instead_of_returning_nothing()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        AddAccount(db, NewCipher(), "aws", "aws-prod",
            new Dictionary<string, string> { ["access_key_id"] = "AKIA1" });

        var (accounts, errors, configured) = await LoadAsync(db, cipher: null);

        Assert.Empty(accounts);
        Assert.Contains("no credential key", Assert.Single(errors), StringComparison.Ordinal);
        Assert.Equal(["aws"], configured);
    }

    [Fact]
    public async Task Several_accounts_of_one_provider_are_all_scanned()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAccount(db, cipher, "aws", "aws-a", new Dictionary<string, string> { ["access_key_id"] = "A" });
        AddAccount(db, cipher, "aws", "aws-b", new Dictionary<string, string> { ["access_key_id"] = "B" });

        var (accounts, errors, _) = await LoadAsync(db, cipher);

        Assert.Equal(2, accounts.Count);
        Assert.Empty(errors);
    }
}
