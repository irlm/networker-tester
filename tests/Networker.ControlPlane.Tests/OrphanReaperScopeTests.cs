using System.Text;
using System.Text.Json;
using Microsoft.Data.Sqlite;
using Microsoft.EntityFrameworkCore;
using Microsoft.Extensions.DependencyInjection;
using Networker.ControlPlane.Background;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// Tests for the cloud orphan-reaper's scope resolution + pure filter/argv logic
/// (<see cref="OrphanReaperService"/>). The headline case: prod provisions testers
/// via <c>cloud_accounts</c>, not <c>cloud_connections</c> — an earlier reaper
/// keyed only on connections and therefore never ran on prod. These tests pin the
/// account-scope resolution (creds decrypted via <see cref="CredentialCipher"/>,
/// RG resolved the same way the provisioner does), the connection/account de-dupe,
/// the honest "nothing configured" note, the <c>name_is_ours</c> allow-list, the
/// known-set exclusion, and the NSG delete-order + argv (the C# divergence from
/// Rust).
/// </summary>
public sealed class OrphanReaperScopeTests
{
    // A fixed 32-byte AES key so encrypt/decrypt round-trips in-test.
    private static CredentialCipher NewCipher() =>
        new(Enumerable.Range(0, 32).Select(i => (byte)i).ToArray());

    // ── Scope resolution against a real Sqlite NetworkerDbContext ─────────────

    private static (NetworkerDbContext Db, SqliteConnection Conn) NewDb()
    {
        var conn = new SqliteConnection("DataSource=:memory:");
        conn.Open();

        var services = new ServiceCollection();
        services.AddDbContext<NetworkerDbContext>(o => o.UseSqlite(conn));
        var sp = services.BuildServiceProvider();

        // Only the three tables the reaper's scope resolution queries — with the
        // real column names. (The full Postgres model can't be built on Sqlite.)
        Exec(conn, """
            CREATE TABLE cloud_connection (
                connection_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                provider TEXT NOT NULL,
                config TEXT NOT NULL,
                status TEXT NOT NULL,
                last_validated TEXT,
                validation_error TEXT,
                created_by TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                project_id TEXT
            );
            """);
        Exec(conn, """
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
            """);
        Exec(conn, """
            CREATE TABLE project_tester (
                tester_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL,
                cloud TEXT NOT NULL,
                region TEXT NOT NULL,
                vm_size TEXT NOT NULL,
                vm_resource_id TEXT,
                ssh_user TEXT NOT NULL,
                power_state TEXT NOT NULL,
                allocation TEXT NOT NULL,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            """);

        return (sp.GetRequiredService<NetworkerDbContext>(), conn);
    }

    private static void Exec(SqliteConnection conn, string sql)
    {
        using var cmd = conn.CreateCommand();
        cmd.CommandText = sql;
        cmd.ExecuteNonQuery();
    }

    private static void AddAzureAccount(
        NetworkerDbContext db, CredentialCipher cipher,
        string subscription, string? resourceGroup, string status = "active",
        bool withServicePrincipal = true)
    {
        var payload = new Dictionary<string, string?>
        {
            ["subscription_id"] = subscription,
        };
        if (resourceGroup is not null)
        {
            payload["resource_group"] = resourceGroup;
        }
        if (withServicePrincipal)
        {
            payload["client_id"] = "client-abc";
            payload["client_secret"] = "secret-xyz";
            payload["tenant_id"] = "tenant-123";
        }

        var plain = Encoding.UTF8.GetBytes(JsonSerializer.Serialize(payload));
        var (enc, nonce) = cipher.Encrypt(plain);

        db.CloudAccounts.Add(new Data.Entities.CloudAccount
        {
            AccountId = Guid.NewGuid(),
            Name = "az-acct",
            Provider = "azure",
            CredentialsEnc = enc,
            CredentialsNonce = nonce,
            Status = status,
            ProjectId = "proj-000000001",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
    }

    private static void AddAzureConnection(NetworkerDbContext db, string subscription, string resourceGroup)
    {
        var config = JsonSerializer.Serialize(new
        {
            subscription_id = subscription,
            resource_group = resourceGroup,
        });
        db.CloudConnections.Add(new Data.Entities.CloudConnection
        {
            ConnectionId = Guid.NewGuid(),
            Name = "az-conn",
            Provider = "azure",
            Config = config,
            Status = "active",
            CreatedAt = DateTime.UtcNow,
            UpdatedAt = DateTime.UtcNow,
        });
        db.SaveChanges();
    }

    [Fact]
    public async Task Scope_from_cloud_account_only_yields_account_scope()
    {
        // The exact prod failure: an active cloud_account, ZERO cloud_connections.
        // The old reaper keyed on connections → no-op → orphans forever.
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAzureAccount(db, cipher, "sub-PROD", "ALETHEDASH-RG");

        var scopes = await OrphanReaperService.ResolveAzureScopesAsync(db, cipher, default);

        var s = Assert.Single(scopes);
        Assert.Equal("sub-PROD", s.Subscription);
        Assert.Equal("ALETHEDASH-RG", s.ResourceGroup);
        Assert.Equal("cloud_account", s.Source);
        // SP creds were present → we'll log in with them, not the ambient identity.
        Assert.NotNull(s.ServicePrincipal);
        Assert.Equal("client-abc", s.ServicePrincipal!.Value.ClientId);
    }

    [Fact]
    public async Task Scope_from_account_without_resource_group_uses_provisioner_default()
    {
        // Matches TesterWriteEndpoints.ResolveProviderCredentialsAsync: absent
        // resource_group claim → "networker-testers", NOT DASHBOARD_AZURE_RG.
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAzureAccount(db, cipher, "sub-1", resourceGroup: null);

        var scopes = await OrphanReaperService.ResolveAzureScopesAsync(db, cipher, default);

        var s = Assert.Single(scopes);
        Assert.Equal(OrphanReaperService.DefaultAzureResourceGroup, s.ResourceGroup);
        Assert.Equal("networker-testers", s.ResourceGroup);
    }

    [Fact]
    public async Task Scope_connection_and_account_same_scope_dedupe_to_one_scan()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        // Same (subscription, resource-group) via both a connection and account —
        // must de-dupe to a single scan. Case differs to prove case-insensitive
        // de-dupe (Azure ids/names are case-insensitive).
        AddAzureConnection(db, "sub-shared", "rg-shared");
        AddAzureAccount(db, cipher, "SUB-SHARED", "RG-SHARED");

        var scopes = await OrphanReaperService.ResolveAzureScopesAsync(db, cipher, default);

        var s = Assert.Single(scopes);
        // Connection is added first → it wins the de-dupe (ambient identity, no SP
        // login needed for a scope the ambient identity already covers).
        Assert.Equal("cloud_connection", s.Source);
        Assert.Null(s.ServicePrincipal);
    }

    [Fact]
    public async Task Scope_neither_configured_yields_empty()
    {
        var (db, conn) = NewDb();
        using var _ = conn;

        var scopes = await OrphanReaperService.ResolveAzureScopesAsync(db, NewCipher(), default);

        Assert.Empty(scopes);
    }

    [Fact]
    public async Task Scope_inactive_account_is_ignored()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        var cipher = NewCipher();
        AddAzureAccount(db, cipher, "sub-inactive", "rg-x", status: "invalid");

        var scopes = await OrphanReaperService.ResolveAzureScopesAsync(db, cipher, default);

        Assert.Empty(scopes);
    }

    [Fact]
    public async Task Scope_null_cipher_skips_account_scopes_but_keeps_connections()
    {
        var (db, conn) = NewDb();
        using var _ = conn;
        AddAzureConnection(db, "sub-conn", "rg-conn");
        // An account exists but without a cipher we can't decrypt it → skipped.
        AddAzureAccount(db, NewCipher(), "sub-acct", "rg-acct");

        var scopes = await OrphanReaperService.ResolveAzureScopesAsync(db, cipher: null, default);

        var s = Assert.Single(scopes);
        Assert.Equal("cloud_connection", s.Source);
    }

    // ── name_is_ours allow-list ───────────────────────────────────────────────

    [Theory]
    [InlineData("tester-eastus-01", true)]
    [InlineData("Tester-EastUS-01", true)] // case-insensitive
    [InlineData("ab-ubuntu-loop-01", true)]
    [InlineData("nwk-ep-eu-west-01", true)]
    [InlineData("prod-app-server", false)] // someone else's VM — never touched
    [InlineData("bastion-nic", false)]
    [InlineData("nwk-something-else", false)] // nwk- alone isn't an owned prefix
    public void NameIsOurs_matches_only_tester_naming(string name, bool expected)
    {
        Assert.Equal(expected, OrphanReaperService.NameIsOurs(name));
    }

    // ── Known-set exclusion + filter ──────────────────────────────────────────

    [Fact]
    public void FilterOrphans_excludes_known_resource_ids_and_foreign_names()
    {
        var known = new HashSet<string>(new[] { "/r/known" }, StringComparer.OrdinalIgnoreCase);
        var raw = new[]
        {
            new OrphanReaperService.RawResource("/r/1", "tester-eastus-01", "vm", "azure"),
            // In project_tester.vm_resource_id → NOT reaped even though owned name.
            new OrphanReaperService.RawResource("/r/known", "tester-known-01", "vm", "azure"),
            // Foreign name → NOT reaped even though unknown id.
            new OrphanReaperService.RawResource("/r/2", "prod-app-server", "vm", "azure"),
            new OrphanReaperService.RawResource("/r/3", "tester-eastus-01NSG", "nsg", "azure"),
        };

        var orphans = OrphanReaperService.FilterOrphans(raw, known);

        Assert.Equal(2, orphans.Count);
        Assert.Contains(orphans, o => o.ResourceId == "/r/1");
        Assert.Contains(orphans, o => o.ResourceId == "/r/3" && o.Kind == "nsg");
        Assert.DoesNotContain(orphans, o => o.ResourceId == "/r/known");
        Assert.DoesNotContain(orphans, o => o.ResourceId == "/r/2");
    }

    // ── NSG: delete order + argv (divergence from Rust) ───────────────────────

    [Fact]
    public void DeleteOrder_places_nsg_after_nic()
    {
        var order = OrphanReaperService.DeleteOrder;
        Assert.Equal(new[] { "vm", "nic", "disk", "public_ip", "nsg" }, order);

        // An NSG can only be deleted once no NIC references it → nsg must come
        // strictly after nic.
        var nicIdx = Array.IndexOf(order, "nic");
        var nsgIdx = Array.IndexOf(order, "nsg");
        Assert.True(nsgIdx > nicIdx, "nsg must be deleted after nic");
    }

    [Fact]
    public void BuildDeleteArgs_nsg_matches_az_network_nsg_delete()
    {
        var args = OrphanReaperService.BuildDeleteArgs("nsg", "/subs/x/nsg/tester-01NSG", "sub-1");

        Assert.Equal(new[]
        {
            "network", "nsg", "delete",
            "--subscription", "sub-1",
            "--ids", "/subs/x/nsg/tester-01NSG",
        }, args);
    }

    [Theory]
    [InlineData("vm", new[] { "vm", "delete", "--subscription", "sub-1", "--ids", "/id", "--yes" })]
    [InlineData("nic", new[] { "network", "nic", "delete", "--subscription", "sub-1", "--ids", "/id" })]
    [InlineData("public_ip", new[] { "network", "public-ip", "delete", "--subscription", "sub-1", "--ids", "/id" })]
    [InlineData("disk", new[] { "disk", "delete", "--subscription", "sub-1", "--ids", "/id", "--yes" })]
    public void BuildDeleteArgs_matches_rust_argv_for_each_kind(string kind, string[] expected)
    {
        Assert.Equal(expected, OrphanReaperService.BuildDeleteArgs(kind, "/id", "sub-1"));
    }

    [Fact]
    public void BuildDeleteArgs_unknown_kind_is_null()
    {
        Assert.Null(OrphanReaperService.BuildDeleteArgs("bucket", "/id", "sub-1"));
    }

    // ── config parsing ────────────────────────────────────────────────────────

    [Fact]
    public void ParseAzureScopeFromConfig_reads_sub_and_rg()
    {
        var (sub, rg) = OrphanReaperService.ParseAzureScopeFromConfig(
            """{"subscription_id":"s","resource_group":"g","other":true}""");
        Assert.Equal("s", sub);
        Assert.Equal("g", rg);
    }

    [Fact]
    public void ParseAzureScopeFromConfig_bad_json_is_null()
    {
        var (sub, rg) = OrphanReaperService.ParseAzureScopeFromConfig("not json");
        Assert.Null(sub);
        Assert.Null(rg);
    }
}
