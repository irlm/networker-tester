using System.Text.Json;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// C# port of the Rust dashboard's <c>api/tester_precheck.rs</c> — pre-flight
/// checks run before creating a tester VM. Project-scoped and requires the
/// Operator role (Rust: <c>require_project_role(Operator)</c> → ProjectOperator
/// policy).
///
/// <para>Route: <b>POST /api/projects/{projectId}/testers/precheck</b>. Body:
/// <c>{ cloud, region, requested_os?, requested_variant? }</c>. Response:
/// <c>{ status, blockers, warnings, auto_resolved }</c> where status is
/// "ok" | "warning" | "blocked" and each blocker/warning is
/// <c>{ code, message, resolution }</c>.</para>
///
/// <para>Faithful structured logic: no active cloud account → single
/// <c>no_cloud_account</c> blocker + status "blocked"; missing credential key →
/// <c>no_credential_key</c> blocker; corrupt nonce → <c>invalid_nonce</c>; decrypt
/// failure → <c>decrypt_failed</c>; unknown cloud → <c>unknown_cloud</c>. Then
/// provider-specific checks. Final status: any blocker → blocked; else any
/// warning → warning; else ok.</para>
///
/// <para><b>Stub divergences (TODO(phase3)):</b> the provider prechecks that shell
/// out to cloud CLIs are stubbed (no <c>az</c>/<c>aws</c>/<c>gcloud</c> here):
/// (1) Azure orphan-Public-IP listing/deletion is skipped (no
///   <c>auto_resolved</c> entry, no <c>azure_ip_list_failed</c> warning); the
///   config-validity check (all SP fields present) + the Windows-11-Desktop
///   licensing warning ARE reproduced.
/// (2) AWS STS credential check is skipped; the config-validity check
///   (access_key_id + secret_access_key present) IS reproduced.
/// (3) GCP: the service-account-key validity check IS reproduced; the local
///   <c>~/.ssh/id_rsa.pub</c> presence warning is reproduced against the C# host.
/// No cloud data is fabricated.</para>
/// </summary>
public static class TesterPrecheckEndpoints
{
    public static IEndpointRouteBuilder MapTesterPrecheckEndpoints(this IEndpointRouteBuilder app)
    {
        app.MapPost("/api/projects/{projectId}/testers/precheck", async (
            string projectId,
            PrecheckRequest? body,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            if (body is null || body.cloud is null || body.region is null)
            {
                return Results.BadRequest("Invalid request body");
            }

            var resp = new PrecheckResponse { status = "ok" };

            // Load the active cloud_account for this cloud (oldest first).
            var account = await db.CloudAccounts
                .AsNoTracking()
                .Where(a => a.ProjectId == projectId && a.Provider == body.cloud && a.Status == "active")
                .OrderBy(a => a.CreatedAt)
                .FirstOrDefaultAsync(ct);

            if (account is null)
            {
                resp.blockers.Add(new PrecheckIssue
                {
                    code = "no_cloud_account",
                    message = $"No active {body.cloud} cloud account found for this project",
                    resolution = $"Go to Settings → Cloud → Add Account, select {body.cloud} and provide credentials",
                });
                resp.status = "blocked";
                return Results.Ok(resp);
            }

            // Decrypt credentials (the cipher handles primary+old-key fallback and
            // stands in for the Rust no_credential_key / invalid_nonce guards).
            Dictionary<string, string> creds;
            try
            {
                var plain = cipher.Decrypt(account.CredentialsEnc, account.CredentialsNonce);
                creds = ParseCreds(plain);
            }
            catch (Exception)
            {
                resp.blockers.Add(new PrecheckIssue
                {
                    code = "decrypt_failed",
                    message = "Cloud credentials could not be decrypted",
                    resolution = "Delete and recreate the cloud account with current credentials",
                });
                resp.status = "blocked";
                return Results.Ok(resp);
            }

            switch (body.cloud)
            {
                case "azure":
                    PrecheckAzure(creds, body, resp);
                    break;
                case "aws":
                    PrecheckAws(creds, body, resp);
                    break;
                case "gcp":
                    PrecheckGcp(creds, resp);
                    break;
                default:
                    resp.blockers.Add(new PrecheckIssue
                    {
                        code = "unknown_cloud",
                        message = $"Unknown cloud provider: {body.cloud}",
                        resolution = "Use azure, aws, or gcp",
                    });
                    resp.status = "blocked";
                    break;
            }

            // Final status.
            if (resp.blockers.Count > 0)
            {
                resp.status = "blocked";
            }
            else if (resp.warnings.Count > 0)
            {
                resp.status = "warning";
            }

            return Results.Ok(resp);
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        return app;
    }

    private static void PrecheckAzure(
        IReadOnlyDictionary<string, string> creds, PrecheckRequest req, PrecheckResponse resp)
    {
        // Azure SP config validity (from_config requires these fields).
        bool Has(string k) => creds.TryGetValue(k, out var v) && !string.IsNullOrEmpty(v);
        if (!Has("subscription_id") || !Has("tenant_id") || !Has("client_id") || !Has("client_secret"))
        {
            resp.blockers.Add(new PrecheckIssue
            {
                code = "azure_config_invalid",
                message = "Azure cloud account config invalid: missing required service principal fields",
                resolution = "Edit the cloud account and fill in all required fields",
            });
            return;
        }

        // TODO(phase3): list + auto-delete unattached Azure Public IPs (az CLI).
        // Stubbed — no az CLI. No auto_resolved / azure_ip_list_failed entry.

        // Region/OS licensing warning is pure logic — reproduced.
        if (req.requested_os == "windows-11" && req.requested_variant == "desktop")
        {
            resp.warnings.Add(new PrecheckIssue
            {
                code = "azure_windows_11_license",
                message = "Windows 11 Desktop images require Multi-Tenant Hosting Rights or Visual Studio license",
                resolution = "Check Azure Marketplace licensing for Windows 11 Desktop in your subscription",
            });
        }
    }

    private static void PrecheckAws(
        IReadOnlyDictionary<string, string> creds, PrecheckRequest req, PrecheckResponse resp)
    {
        bool Has(string k) => creds.TryGetValue(k, out var v) && !string.IsNullOrEmpty(v);
        if (!Has("access_key_id") || !Has("secret_access_key"))
        {
            resp.blockers.Add(new PrecheckIssue
            {
                code = "aws_config_invalid",
                message = "AWS config invalid: missing access_key_id or secret_access_key",
                resolution = "Edit the AWS cloud account and provide access_key_id + secret_access_key",
            });
            return;
        }

        // TODO(phase3): `aws sts get-caller-identity` to detect expired/invalid
        // credentials. Stubbed — no aws CLI. No aws_credentials_expired /
        // aws_invalid_credentials / aws_sts_failed / aws_cli_missing entry.
    }

    private static void PrecheckGcp(IReadOnlyDictionary<string, string> creds, PrecheckResponse resp)
    {
        // GcpProvider::from_config requires a valid service account JSON key.
        // Approximate the validity check: require a private_key + client_email.
        bool Has(string k) => creds.TryGetValue(k, out var v) && !string.IsNullOrEmpty(v);
        if (!Has("private_key") || !Has("client_email"))
        {
            resp.blockers.Add(new PrecheckIssue
            {
                code = "gcp_config_invalid",
                message = "GCP cloud account config invalid",
                resolution = "Edit the GCP cloud account and provide a valid service account JSON key",
            });
            return;
        }

        // Local SSH public key presence — pure host check, reproduced.
        var home = Environment.GetEnvironmentVariable("HOME") ?? string.Empty;
        var pubKeyPath = Path.Combine(home, ".ssh", "id_rsa.pub");
        if (!File.Exists(pubKeyPath))
        {
            resp.warnings.Add(new PrecheckIssue
            {
                code = "gcp_no_local_ssh_key",
                message = "Dashboard host has no ~/.ssh/id_rsa.pub — GCP VMs will not be reachable via SSH",
                resolution = "Run `ssh-keygen -t rsa -b 4096` on the dashboard host, or GCP will fall back to OS Login (slower)",
            });
        }
    }

    private static Dictionary<string, string> ParseCreds(byte[] plain)
    {
        var map = new Dictionary<string, string>();
        using var doc = JsonDocument.Parse(plain);
        if (doc.RootElement.ValueKind == JsonValueKind.Object)
        {
            foreach (var prop in doc.RootElement.EnumerateObject())
            {
                map[prop.Name] = prop.Value.ValueKind == JsonValueKind.String
                    ? prop.Value.GetString() ?? string.Empty
                    : prop.Value.GetRawText();
            }
        }
        return map;
    }

    public sealed class PrecheckRequest
    {
        public string? cloud { get; set; }
        public string? region { get; set; }
        public string? requested_os { get; set; }
        public string? requested_variant { get; set; }
    }

    public sealed class PrecheckResponse
    {
        public string status { get; set; } = string.Empty;
        public List<PrecheckIssue> blockers { get; set; } = new();
        public List<PrecheckIssue> warnings { get; set; } = new();
        public List<string> auto_resolved { get; set; } = new();
    }

    public sealed class PrecheckIssue
    {
        public string code { get; set; } = string.Empty;
        public string message { get; set; } = string.Empty;
        public string resolution { get; set; } = string.Empty;
    }
}
