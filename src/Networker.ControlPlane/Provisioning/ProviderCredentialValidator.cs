using System.Diagnostics;
using System.Text.Json;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Real provider credential validation — the C# port of the Rust
/// dashboard's validators, which the phase-3 port had STUBBED to a
/// field-presence check: an expired AWS token validated "active" because
/// the keys existed, not because they worked (user-caught 2026-08-11,
/// the same reader-without-writer port-fidelity family as the
/// system-health checker).
///
/// <list type="bullet">
///   <item><b>azure</b> — client-credentials token request straight to
///   login.microsoftonline.com (no CLI, no session pollution), with the
///   Rust version's friendly AADSTS error mapping.</item>
///   <item><b>aws</b> — <c>aws sts get-caller-identity</c> with the keys
///   passed as environment variables (honors <c>session_token</c>).</item>
///   <item><b>gcp</b> — <c>gcloud auth activate-service-account</c>
///   against an ISOLATED <c>CLOUDSDK_CONFIG</c> temp dir — the Rust
///   version activated into the host's shared gcloud config, silently
///   switching the machine's active account (fixed in this port).</item>
/// </list>
///
/// Hosts without the aws/gcloud CLIs get an honest "CLI not available"
/// error instead of a fake pass.
/// </summary>
public interface IProviderCredentialValidator
{
    Task<(string Status, string? Error)> ValidateAsync(
        string provider,
        IReadOnlyDictionary<string, string> creds,
        CancellationToken ct);
}

public sealed class ProviderCredentialValidator : IProviderCredentialValidator
{
    private static readonly TimeSpan Timeout = TimeSpan.FromSeconds(15);

    private readonly IHttpClientFactory _httpFactory;
    private readonly ILogger<ProviderCredentialValidator> _logger;

    public ProviderCredentialValidator(
        IHttpClientFactory httpFactory,
        ILogger<ProviderCredentialValidator> logger)
    {
        _httpFactory = httpFactory;
        _logger = logger;
    }

    public async Task<(string Status, string? Error)> ValidateAsync(
        string provider,
        IReadOnlyDictionary<string, string> creds,
        CancellationToken ct)
    {
        string Get(string k) => creds.TryGetValue(k, out var v) ? v : "";

        switch (provider)
        {
            case "azure":
            {
                var clientId = Get("client_id");
                var clientSecret = Get("client_secret");
                var tenantId = Get("tenant_id");
                if (clientId.Length == 0 || clientSecret.Length == 0 || tenantId.Length == 0)
                {
                    return ("error", "Missing client_id, client_secret, or tenant_id");
                }
                return await ValidateAzureAsync(clientId, clientSecret, tenantId, ct).ConfigureAwait(false);
            }
            case "aws":
            {
                var accessKey = Get("access_key_id");
                var secretKey = Get("secret_access_key");
                if (accessKey.Length == 0 || secretKey.Length == 0)
                {
                    return ("error", "Missing access_key_id or secret_access_key");
                }
                return await ValidateAwsAsync(accessKey, secretKey, Get("session_token"), ct).ConfigureAwait(false);
            }
            case "gcp":
            {
                var jsonKey = Get("json_key");
                if (jsonKey.Length == 0)
                {
                    return ("error", "Missing json_key");
                }
                return await ValidateGcpAsync(jsonKey, ct).ConfigureAwait(false);
            }
            default:
                return ("error", $"Unknown provider: {provider}");
        }
    }

    private async Task<(string, string?)> ValidateAzureAsync(
        string clientId, string clientSecret, string tenantId, CancellationToken ct)
    {
        var client = _httpFactory.CreateClient(nameof(ProviderCredentialValidator));
        client.Timeout = Timeout;
        try
        {
            using var resp = await client.PostAsync(
                $"https://login.microsoftonline.com/{Uri.EscapeDataString(tenantId)}/oauth2/v2.0/token",
                new FormUrlEncodedContent(new Dictionary<string, string>
                {
                    ["grant_type"] = "client_credentials",
                    ["client_id"] = clientId,
                    ["client_secret"] = clientSecret,
                    ["scope"] = "https://management.azure.com/.default",
                }),
                ct).ConfigureAwait(false);

            var raw = await resp.Content.ReadAsStringAsync(ct).ConfigureAwait(false);
            JsonElement body;
            try
            {
                body = JsonDocument.Parse(raw).RootElement.Clone();
            }
            catch (JsonException)
            {
                body = default;
            }

            if (resp.IsSuccessStatusCode
                && body.ValueKind == JsonValueKind.Object
                && body.TryGetProperty("access_token", out _))
            {
                return ("active", null);
            }

            var errorDesc = body.ValueKind == JsonValueKind.Object
                && body.TryGetProperty("error_description", out var d) ? d.GetString() ?? "" : "";
            var errorCode = body.ValueKind == JsonValueKind.Object
                && body.TryGetProperty("error", out var e) ? e.GetString() ?? "" : "";
            return ("error", MapAzureError(errorCode, errorDesc, clientId, tenantId, (int)resp.StatusCode));
        }
        catch (Exception ex) when (ex is not OperationCanceledException || !ct.IsCancellationRequested)
        {
            return ("error", $"Could not reach Azure AD: {ex.Message}");
        }
    }

    /// <summary>Rust-parity friendly messages for the common AADSTS failures.</summary>
    internal static string MapAzureError(string errorCode, string errorDesc, string clientId, string tenantId, int httpStatus)
    {
        if (errorCode == "invalid_client" || errorDesc.Contains("AADSTS7000215"))
        {
            return "Invalid client secret. Generate a new one: Azure Portal → App registrations → Certificates & secrets.";
        }
        if (errorDesc.Contains("AADSTS700016") || errorDesc.Contains("not found in the directory"))
        {
            return $"Application {clientId} not found in tenant {tenantId}. Check the Client ID and Tenant ID.";
        }
        if (errorDesc.Contains("AADSTS90002") || errorDesc.Contains("not found"))
        {
            return $"Tenant '{tenantId}' not found. Check the Tenant ID.";
        }
        if (errorDesc.Length > 0)
        {
            return $"Azure: {errorDesc.Split('\r')[0]}";
        }
        return $"Azure token request failed (HTTP {httpStatus})";
    }

    private async Task<(string, string?)> ValidateAwsAsync(
        string accessKey, string secretKey, string sessionToken, CancellationToken ct)
    {
        var psi = new ProcessStartInfo
        {
            FileName = "aws",
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        psi.ArgumentList.Add("sts");
        psi.ArgumentList.Add("get-caller-identity");
        psi.Environment["AWS_ACCESS_KEY_ID"] = accessKey;
        psi.Environment["AWS_SECRET_ACCESS_KEY"] = secretKey;
        if (sessionToken.Length > 0)
        {
            psi.Environment["AWS_SESSION_TOKEN"] = sessionToken;
        }

        var (status, error) = await RunCliAsync(psi, "AWS validation failed", "aws CLI not available", ct).ConfigureAwait(false);
        return error is null ? (status, error) : (status, CleanAwsError(error));
    }

    /// <summary>
    /// The aws CLI wraps the useful part in three layers of boilerplate
    /// ("aws: [ERROR]: An error occurred (Code) when calling the Op
    /// operation: message") and the UI truncates from the LEFT — so the
    /// visible fragment carried zero signal (user screenshot 2026-08-12).
    /// Extract the code + message and lead with the human explanation.
    /// </summary>
    internal static string CleanAwsError(string raw)
    {
        var m = System.Text.RegularExpressions.Regex.Match(
            raw, @"An error occurred \((?<code>\w+)\) when calling the \w+ operation:\s*(?<msg>.+?)\s*$",
            System.Text.RegularExpressions.RegexOptions.Singleline);
        if (!m.Success)
        {
            return raw;
        }
        var code = m.Groups["code"].Value;
        var msg = m.Groups["msg"].Value.TrimEnd('.');
        return code switch
        {
            "InvalidClientTokenId" => $"Invalid access key ID — it does not exist or is deactivated ({code}).",
            "SignatureDoesNotMatch" => $"Secret access key does not match the access key ID ({code}).",
            "ExpiredToken" => $"Temporary credentials have expired — generate a fresh session token ({code}).",
            "AccessDenied" => $"Key is valid but denied sts:GetCallerIdentity ({code}): {msg}.",
            _ => $"{code}: {msg}.",
        };
    }

    private async Task<(string, string?)> ValidateGcpAsync(string jsonKey, CancellationToken ct)
    {
        // Isolated config dir: activating a service account must never touch
        // the host's shared gcloud session (the Rust version did — flaw).
        var configDir = Path.Combine(Path.GetTempPath(), $"gcp-validate-{Guid.NewGuid():N}");
        var keyPath = Path.Combine(configDir, "key.json");
        try
        {
            Directory.CreateDirectory(configDir);
            await File.WriteAllTextAsync(keyPath, jsonKey, ct).ConfigureAwait(false);

            var psi = new ProcessStartInfo
            {
                FileName = "gcloud",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            };
            psi.ArgumentList.Add("auth");
            psi.ArgumentList.Add("activate-service-account");
            psi.ArgumentList.Add("--key-file");
            psi.ArgumentList.Add(keyPath);
            psi.Environment["CLOUDSDK_CONFIG"] = configDir;

            var (status, error) = await RunCliAsync(psi, "GCP validation failed", "gcloud CLI not available", ct).ConfigureAwait(false);
            return error is null ? (status, error) : (status, CleanGcloudError(error));
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            return ("error", $"Failed to prepare GCP validation: {ex.Message}");
        }
        finally
        {
            try { Directory.Delete(configDir, recursive: true); } catch { /* best effort */ }
        }
    }

    /// <summary>Strip gcloud's "ERROR: (gcloud.auth.activate-service-account)"
    /// prefix so the visible head of the message is the reason.</summary>
    internal static string CleanGcloudError(string raw)
    {
        var cleaned = System.Text.RegularExpressions.Regex.Replace(
            raw, @"ERROR:\s*\(gcloud[\w.-]*\)\s*", "");
        return cleaned.Length > 0 ? cleaned : raw;
    }

    private async Task<(string, string?)> RunCliAsync(
        ProcessStartInfo psi, string failPrefix, string missingCliMessage, CancellationToken ct)
    {
        try
        {
            using var proc = Process.Start(psi);
            if (proc is null)
            {
                return ("error", missingCliMessage);
            }
            using var timeoutCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
            timeoutCts.CancelAfter(Timeout);
            var stderrTask = proc.StandardError.ReadToEndAsync(timeoutCts.Token);
            _ = proc.StandardOutput.ReadToEndAsync(timeoutCts.Token); // drain to avoid pipe-full deadlock
            await proc.WaitForExitAsync(timeoutCts.Token).ConfigureAwait(false);
            if (proc.ExitCode == 0)
            {
                return ("active", null);
            }
            var stderr = (await stderrTask.ConfigureAwait(false)).Trim();
            return ("error", $"{failPrefix}: {stderr}");
        }
        catch (OperationCanceledException) when (!ct.IsCancellationRequested)
        {
            return ("error", $"{failPrefix}: timed out after {Timeout.TotalSeconds:0}s");
        }
        catch (System.ComponentModel.Win32Exception ex)
        {
            _logger.LogWarning("Validator CLI missing: {Message}", ex.Message);
            return ("error", missingCliMessage);
        }
    }
}
