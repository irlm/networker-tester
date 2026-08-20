using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.Json.Serialization;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Security;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// The <b>create</b> half of the SDK Endpoints page. Until v0.28.274 the page
/// could list SDK endpoints and show the reference samples, but the only way to
/// get one was to deploy a sample app yourself and paste its URL back in — so
/// the page showed samples nobody could create.
///
/// <para>This module provisions them. Three routes under
/// <c>/api/projects/{projectId}/sdk-endpoints/samples</c>:</para>
/// <list type="bullet">
///   <item><b>GET</b> — the catalog (<see cref="SdkSampleCatalog"/>) joined to
///     what this project already has: for each language the deployment carrying
///     it, its host/provider/region/size/cost, and the honest state from
///     <see cref="SdkSamplePlan"/> (current / outdated / unhealthy / nothing).
///     Optional <c>provider</c>/<c>region</c>/<c>vm_size</c> query parameters
///     add the per-shape cost preview so the UI can show what consolidated vs
///     separated actually costs BEFORE anything is provisioned.</item>
///   <item><b>POST</b> — create. Idempotent and reuse-first: every language
///     that already has a usable sample is registered against the existing
///     server (no VM, no money), and only the rest are provisioned — as ONE
///     deployment carrying every remaining language (<c>consolidated</c>) or
///     one deployment per language (<c>separated</c>).</item>
///   <item><b>POST .../{language}/update</b> — re-run the existing deployment
///     in place so an outdated sample picks up the current SDK. Never silently
///     triggered by a create.</item>
/// </list>
///
/// <para><b>How "already exists" is decided:</b> a deployment belongs to a
/// language when its config declares that language in
/// <c>endpoints[i].sdk_samples</c>; the serving host is
/// <c>endpoint_hosts[i] ?? endpoint_ips[i]</c> and the port comes from the
/// catalog. <b>How "outdated" is decided:</b> the running sample is asked —
/// <c>GET {prefix}/health</c> returns <c>sdk.version</c> per contract v1 — and
/// that is compared against the catalog's <c>sdk_version</c> for the language.
/// Nothing is inferred from the repo release version: each SDK versions on its
/// own schedule.</para>
///
/// <para><b>The sample token</b> (<c>X-LagHound-Token</c>) is minted here, once
/// per create, and travels to the installer as AES-256-GCM ciphertext inside
/// the deploy config (<c>sdk_samples.token_enc</c>/<c>token_nonce</c>, the same
/// <see cref="CredentialCipher"/> cloud credentials use). The plaintext never
/// touches the config, the deploy log, or any response body — the runner
/// decrypts it straight into install.sh's environment
/// (<see cref="SdkSampleInstallerToken"/>), and registration copies the same
/// ciphertext onto the SDK endpoint row.</para>
/// </summary>
public static class SdkSampleEndpoints
{
    /// <summary>Deploy-config key carrying the per-endpoint sample list.</summary>
    public const string ConfigKey = "sdk_samples";

    /// <summary>Top-level deploy-config object carrying the encrypted token.</summary>
    public const string TokenConfigKey = "sdk_samples";

    /// <summary>Only Linux endpoints run the samples (every sample builds and
    /// runs on Linux; there is no Windows path in install.sh for them).</summary>
    private const string SampleOs = "linux";

    /// <summary>Health-probe budget per sample host. A dead host must not stall
    /// the page: the whole GET fans out and every probe shares this ceiling.</summary>
    private static readonly TimeSpan ProbeTimeout = TimeSpan.FromMilliseconds(2000);

    public static IEndpointRouteBuilder MapSdkSampleEndpoints(this IEndpointRouteBuilder app)
    {
        // GET /api/projects/{projectId}/sdk-endpoints/samples
        app.MapGet("/api/projects/{projectId}/sdk-endpoints/samples", async (
            string projectId,
            string? provider,
            string? region,
            string? vm_size,
            NetworkerDbContext db,
            CredentialCipher cipher,
            CancellationToken ct) =>
        {
            var view = await BuildViewAsync(projectId, db, cipher, ct).ConfigureAwait(false);
            var cost = await BuildCostPreviewAsync(db, provider, region, vm_size, ct).ConfigureAwait(false);

            return Results.Ok(new
            {
                catalog = SdkSampleCatalog.ToWire(),
                samples = view.Select(v => v.ToWire()).ToArray(),
                cost_preview = cost,
            });
        }).RequireAuthorization(AuthPolicies.ProjectMember);

        // POST /api/projects/{projectId}/sdk-endpoints/samples
        app.MapPost("/api/projects/{projectId}/sdk-endpoints/samples", async (
            string projectId,
            [FromBody] CreateSamplesRequest req,
            HttpContext http,
            NetworkerDbContext db,
            CredentialCipher cipher,
            DeployRunner runner,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var user = http.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }
            if (req is null)
            {
                return ApiError.BadRequest("a request body is required");
            }

            var shape = SdkSamplePlan.ParseShape(req.Shape);
            if (shape is null)
            {
                return ApiError.BadRequest("shape must be 'consolidated' or 'separated'");
            }

            var languages = SdkSampleCatalog.NormalizeLanguages(req.Languages, out var unknown);
            if (unknown.Count > 0)
            {
                return ApiError.BadRequest(
                    $"unknown sample language(s): {string.Join(", ", unknown)}. "
                    + $"Valid: {string.Join(", ", SdkSampleCatalog.Samples.Select(s => s.Id))}");
            }
            if (languages.Count == 0)
            {
                return ApiError.BadRequest("select at least one sample language");
            }

            var view = await BuildViewAsync(projectId, db, cipher, ct).ConfigureAwait(false);
            var verdicts = view.ToDictionary(v => v.Sample.Id, v => v.Verdict, StringComparer.Ordinal);
            var (reuse, provision) = SdkSamplePlan.Split(languages, verdicts, req.ReuseExisting ?? true);

            // A language whose deployment is still in flight is neither
            // reusable nor safe to provision again — refuse rather than
            // double-spend while the first one lands.
            var inFlight = provision
                .Where(l => verdicts.TryGetValue(l, out var v) && v.StateOf == SdkSamplePlan.State.Deploying)
                .ToList();
            if (inFlight.Count > 0)
            {
                return ApiError.Conflict(
                    $"a deployment for {string.Join(", ", inFlight)} is still running — wait for it to finish, "
                    + "then reuse it instead of provisioning a second server");
            }

            var byId = view.ToDictionary(v => v.Sample.Id, StringComparer.Ordinal);

            // ── Reuse: register an SDK endpoint against what already serves ──
            var registered = new List<object>();
            foreach (var lang in reuse)
            {
                var entry = byId[lang];
                var existing = entry.Deployed?.RegisteredEndpointId;
                if (existing is not null)
                {
                    registered.Add(new
                    {
                        language = lang,
                        sdk_endpoint_id = existing,
                        url = entry.Url,
                        created = false,
                        deployment_id = entry.Deployed!.DeploymentId,
                    });
                    continue;
                }

                var cfg = await RegisterAsync(projectId, entry, user.UserId, db, ct).ConfigureAwait(false);
                if (cfg is null)
                {
                    return ApiError.Conflict(
                        $"an SDK endpoint named '{RegistrationName(entry.Sample)}' already exists — rename or delete it, then retry");
                }
                registered.Add(new
                {
                    language = lang,
                    sdk_endpoint_id = cfg.Id,
                    url = entry.Url,
                    created = true,
                    deployment_id = entry.Deployed!.DeploymentId,
                });
            }

            // ── Provision: one deployment (consolidated) or N (separated) ────
            var created = new List<object>();
            if (provision.Count > 0)
            {
                if (!TryResolveTarget(req, out var target, out var targetError))
                {
                    return ApiError.BadRequest(targetError!);
                }

                // One token per create request; every sample it provisions
                // shares it, exactly like examples/docker-compose.yml.
                var token = MintToken();
                var (enc, nonce) = cipher.Encrypt(Encoding.UTF8.GetBytes(token));
                var tokenNode = new JsonObject
                {
                    ["token_enc"] = Convert.ToBase64String(enc),
                    ["token_nonce"] = Convert.ToBase64String(nonce),
                };

                foreach (var group in GroupForShape(target, provision))
                {
                    var name = DeploymentName(group);
                    var config = BuildDeployConfig(target, group, tokenNode);
                    var (id, error) = await DeploymentWriteEndpoints
                        .CreateAndSpawnAsync(projectId, name, config, user.UserId, db, runner, loggerFactory, ct)
                        .ConfigureAwait(false);
                    if (id is null)
                    {
                        return ApiError.Status(StatusCodes.Status422UnprocessableEntity, error ?? "deploy config rejected");
                    }
                    created.Add(new
                    {
                        deployment_id = id,
                        name,
                        languages = group.ToArray(),
                        provider = target.Provider,
                        region = target.Region,
                        vm_size = target.VmSize,
                    });
                }
            }

            return Results.Ok(new
            {
                shape = req.Shape?.Trim().ToLowerInvariant(),
                reused = registered.ToArray(),
                deployments = created.ToArray(),
                // What the caller must still do: a provisioned sample can only
                // be registered once its deploy reports a host. The UI re-POSTs
                // (reuse-first) when the deployment completes.
                pending_registration = provision.ToArray(),
                servers_provisioned = created.Count,
                servers_avoided = reuse.Count,
            });
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        // POST /api/projects/{projectId}/sdk-endpoints/samples/{language}/update
        app.MapPost("/api/projects/{projectId}/sdk-endpoints/samples/{language}/update", async (
            string projectId,
            string language,
            NetworkerDbContext db,
            CredentialCipher cipher,
            DeployRunner runner,
            ILoggerFactory loggerFactory,
            CancellationToken ct) =>
        {
            var sample = SdkSampleCatalog.Find(language);
            if (sample is null)
            {
                return ApiError.NotFound($"unknown sample language '{language}'");
            }

            var view = await BuildViewAsync(projectId, db, cipher, ct).ConfigureAwait(false);
            var entry = view.FirstOrDefault(v => v.Sample.Id == sample.Id);
            if (entry?.Deployed is null)
            {
                return ApiError.NotFound($"no deployment in this project carries the {sample.Language} sample");
            }
            if (SdkSamplePlan.IsInFlight(entry.Deployed.DeploymentStatus))
            {
                return ApiError.Conflict(
                    $"deployment {entry.Deployed.DeploymentName} is {entry.Deployed.DeploymentStatus} — wait for it to finish");
            }

            var deployment = await db.Deployments
                .AsNoTracking()
                .FirstOrDefaultAsync(d => d.ProjectId == projectId && d.DeploymentId == entry.Deployed.DeploymentId, ct)
                .ConfigureAwait(false);
            if (deployment is null)
            {
                return ApiError.NotFound("deployment not found");
            }

            // Re-run the stored config with tests off — the same in-place path
            // the deployment /update button uses. install.sh's sample setup is
            // idempotent: it rebuilds the sample from the current source and
            // restarts the service, so the host keeps its address and cost.
            var config = JsonNode.Parse(deployment.Config) as JsonObject ?? new JsonObject();
            config["tests"] = new JsonObject { ["run_tests"] = false };
            DeploymentWriteEndpoints.SpawnDeployPublic(runner, loggerFactory, deployment.DeploymentId, config.ToJsonString());

            return Results.Accepted(
                $"/api/projects/{projectId}/deployments/{deployment.DeploymentId}",
                new
                {
                    status = "updating",
                    deployment_id = deployment.DeploymentId,
                    language = sample.Id,
                    from_version = entry.Verdict.DeployedVersion,
                    to_version = sample.SdkVersion,
                });
        }).RequireAuthorization(AuthPolicies.ProjectOperator);

        return app;
    }

    // ── The per-language view (discovery + probe + verdict) ───────────────────

    /// <summary>One catalog language joined to what the project has for it.</summary>
    internal sealed record SampleView(
        SdkSampleCatalog.Sample Sample,
        SdkSamplePlan.Deployed? Deployed,
        string? Url,
        SdkSamplePlan.Verdict Verdict)
    {
        public object ToWire() => new
        {
            language = Sample.Id,
            label = Sample.Language,
            runtime = Sample.Runtime,
            description = Sample.Description,
            port = Sample.Port,
            url = Url,
            route = SdkSampleCatalog.RouteDefault,
            state = SdkSamplePlan.Wire(Verdict.StateOf),
            recommended_action = SdkSamplePlan.Wire(Verdict.Recommended),
            reason = Verdict.Reason,
            reusable = Verdict.Reusable,
            current_version = Verdict.CurrentVersion,
            deployed_version = Verdict.DeployedVersion,
            deployment_id = Deployed?.DeploymentId,
            deployment_name = Deployed?.DeploymentName,
            deployment_status = Deployed?.DeploymentStatus,
            host = Deployed?.Host,
            provider = Deployed?.Provider,
            region = Deployed?.Region,
            vm_size = Deployed?.VmSize,
            consolidated = Deployed?.Consolidated,
            samples_on_host = Deployed?.SampleCountOnHost,
            sdk_endpoint_id = Deployed?.RegisteredEndpointId,
        };
    }

    /// <summary>
    /// Build the whole per-language view: discover sample deployments, resolve
    /// the token that can read their health, probe them concurrently, and run
    /// each through <see cref="SdkSamplePlan.Decide"/>.
    /// </summary>
    internal static async Task<List<SampleView>> BuildViewAsync(
        string projectId, NetworkerDbContext db, CredentialCipher cipher, CancellationToken ct)
    {
        var deployments = await db.Deployments
            .AsNoTracking()
            .Where(d => d.ProjectId == projectId)
            .OrderByDescending(d => d.CreatedAt)
            .Select(d => new DeploymentRow(
                d.DeploymentId, d.Name, d.Status, d.Config, d.EndpointIps, d.EndpointHosts, d.CreatedAt))
            .ToListAsync(ct)
            .ConfigureAwait(false);

        var hosted = DiscoverSampleHosts(deployments);

        // Registrations: SDK endpoint configs this module created, keyed by
        // (deployment, language) through the endpoint_ref breadcrumb.
        var configs = await db.TestConfigs
            .AsNoTracking()
            .Where(c => c.ProjectId == projectId)
            .ToListAsync(ct)
            .ConfigureAwait(false);
        var registrations = new Dictionary<string, Guid>(StringComparer.Ordinal);
        foreach (var cfg in configs)
        {
            var link = ReadSampleLink(cfg.EndpointRef);
            if (link is not null)
            {
                registrations.TryAdd(LinkKey(link.Value.DeploymentId, link.Value.Language), cfg.Id);
            }
        }

        var views = new List<SampleView>(SdkSampleCatalog.Samples.Count);
        var probes = new Dictionary<string, Task<SdkSamplePlan.Probe>>(StringComparer.Ordinal);

        foreach (var sample in SdkSampleCatalog.Samples)
        {
            if (!hosted.TryGetValue(sample.Id, out var host))
            {
                continue;
            }
            if (string.IsNullOrWhiteSpace(host.Host) || !host.Terminal)
            {
                continue;
            }
            var token = ResolveToken(cipher, host.ConfigJson, configs, host.DeploymentId, sample.Id);
            probes[sample.Id] = ProbeSampleAsync(host.Host!, sample.Port, token, ct);
        }

        await Task.WhenAll(probes.Values).ConfigureAwait(false);

        foreach (var sample in SdkSampleCatalog.Samples)
        {
            SdkSamplePlan.Deployed? deployed = null;
            string? url = null;
            if (hosted.TryGetValue(sample.Id, out var host))
            {
                registrations.TryGetValue(LinkKey(host.DeploymentId, sample.Id), out var regId);
                deployed = new SdkSamplePlan.Deployed(
                    host.DeploymentId,
                    host.DeploymentName,
                    host.Status,
                    host.Host,
                    host.Provider,
                    host.Region,
                    host.VmSize,
                    host.Languages.Count > 1,
                    host.Languages.Count,
                    regId == Guid.Empty ? null : regId);
                url = host.Host is null ? null : SampleUrl(host.Host, sample.Port);
            }

            var probe = probes.TryGetValue(sample.Id, out var task) ? task.Result : null;
            views.Add(new SampleView(sample, deployed, url, SdkSamplePlan.Decide(sample, deployed, probe)));
        }

        return views;
    }

    /// <summary>The deployment columns discovery needs (kept narrow so the
    /// query never drags whole logs into memory).</summary>
    internal sealed record DeploymentRow(
        Guid DeploymentId, string Name, string Status,
        string Config, string? EndpointIps, string? EndpointHosts, DateTime CreatedAt);

    /// <summary>One host serving one or more samples.</summary>
    internal sealed record SampleHost(
        Guid DeploymentId,
        string DeploymentName,
        string Status,
        string? Host,
        string Provider,
        string? Region,
        string? VmSize,
        IReadOnlyList<string> Languages,
        string ConfigJson,
        bool Terminal);

    /// <summary>
    /// Which deployment currently owns each language: newest-first scan over
    /// the project's deployments, first hit wins — a language's state is the
    /// state of its most recent sample deployment, never a stale older one.
    /// Pure (no DB/HTTP) so the discovery rules are unit-testable.
    /// </summary>
    internal static Dictionary<string, SampleHost> DiscoverSampleHosts(IReadOnlyList<DeploymentRow> deployments)
    {
        var found = new Dictionary<string, SampleHost>(StringComparer.Ordinal);
        foreach (var d in deployments.OrderByDescending(d => d.CreatedAt))
        {
            var specs = DeploymentsEndpoints.ParseEndpointSpecs(d.Config);
            var ips = ParseStringArray(d.EndpointIps);
            var hosts = ParseStringArray(d.EndpointHosts);
            var index = -1;
            foreach (var languages in ParseEndpointSampleLists(d.Config))
            {
                index++;
                if (languages.Count == 0)
                {
                    continue;
                }
                var spec = index < specs.Count ? specs[index] : null;
                var host = At(hosts, index) ?? At(ips, index);
                var terminal = d.Status is "completed" or "failed" or "cancelled" or "torn_down";
                var entry = new SampleHost(
                    d.DeploymentId, d.Name, d.Status, host,
                    spec?.Provider ?? "unknown", spec?.Region, spec?.VmSize,
                    languages, d.Config, terminal);
                foreach (var lang in languages)
                {
                    found.TryAdd(lang, entry);
                }
            }
        }
        return found;
    }

    /// <summary>Per-endpoint <c>sdk_samples</c> lists, in endpoint order (empty
    /// list for endpoints that declare none). Tolerant: bad JSON ⇒ nothing.</summary>
    internal static List<List<string>> ParseEndpointSampleLists(string? rawConfig)
    {
        var result = new List<List<string>>();
        if (string.IsNullOrWhiteSpace(rawConfig))
        {
            return result;
        }
        JsonNode? root;
        try
        {
            root = JsonNode.Parse(rawConfig);
        }
        catch (JsonException)
        {
            return result;
        }
        if (root?["endpoints"] is not JsonArray endpoints)
        {
            return result;
        }
        foreach (var node in endpoints)
        {
            var langs = new List<string>();
            if (node is JsonObject ep && ep[ConfigKey] is JsonArray arr)
            {
                foreach (var item in arr)
                {
                    if (item is JsonValue v && v.TryGetValue<string>(out var s)
                        && SdkSampleCatalog.Find(s) is { } sample)
                    {
                        langs.Add(sample.Id);
                    }
                }
            }
            result.Add(langs);
        }
        return result;
    }

    // ── Health probe ─────────────────────────────────────────────────────────

    /// <summary>
    /// Ask a deployed sample what it is: <c>GET http://{host}:{port}{prefix}/health</c>
    /// with the LagHound token. Contract v1 answers
    /// <c>{"contract":"v1","sdk":{"lang":…,"version":…}}</c>; without a valid
    /// token it answers a bare 404 by design, which we report as reachable with
    /// no version rather than pretending it is healthy-and-current.
    /// </summary>
    internal static async Task<SdkSamplePlan.Probe> ProbeSampleAsync(
        string host, int port, string? token, CancellationToken ct)
    {
        using var client = new HttpClient { Timeout = ProbeTimeout };
        var url = $"http://{host}:{port}{SdkSampleCatalog.PrefixDefault}/health";
        try
        {
            using var req = new HttpRequestMessage(HttpMethod.Get, url);
            if (!string.IsNullOrEmpty(token))
            {
                req.Headers.TryAddWithoutValidation("X-LagHound-Token", token);
            }
            using var resp = await client.SendAsync(req, ct).ConfigureAwait(false);
            if (!resp.IsSuccessStatusCode)
            {
                // 404 with no token is the contract's invisibility, not death:
                // something answered on the port, we just cannot read it.
                return new SdkSamplePlan.Probe(true, null, null);
            }
            var body = await resp.Content.ReadAsStringAsync(ct).ConfigureAwait(false);
            using var doc = JsonDocument.Parse(body);
            string? version = null;
            string? lang = null;
            if (doc.RootElement.TryGetProperty("sdk", out var sdk) && sdk.ValueKind == JsonValueKind.Object)
            {
                if (sdk.TryGetProperty("version", out var v) && v.ValueKind == JsonValueKind.String)
                {
                    version = v.GetString();
                }
                if (sdk.TryGetProperty("lang", out var l) && l.ValueKind == JsonValueKind.String)
                {
                    lang = l.GetString();
                }
            }
            return new SdkSamplePlan.Probe(true, version, lang);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception)
        {
            // Unreachable / timed out / not JSON — all "not serving".
            return new SdkSamplePlan.Probe(false, null, null);
        }
    }

    // ── Registration ─────────────────────────────────────────────────────────

    /// <summary>Deterministic SDK endpoint name for a sample registration.</summary>
    internal static string RegistrationName(SdkSampleCatalog.Sample sample) =>
        $"LagHound {sample.Language} sample";

    private static async Task<Data.Entities.TestConfig?> RegisterAsync(
        string projectId, SampleView entry, Guid? userId, NetworkerDbContext db, CancellationToken ct)
    {
        var deployed = entry.Deployed!;
        var url = entry.Url!;
        var now = DateTime.UtcNow;
        var cfg = new Data.Entities.TestConfig
        {
            Id = Guid.NewGuid(),
            ProjectId = projectId,
            Name = RegistrationName(entry.Sample),
            Description = $"{entry.Sample.Runtime} SDK sample on {deployed.Host} ({deployed.Provider})",
            EndpointKind = "network",
            TestKind = TestConfigKinds.SdkProbe,
            EndpointRef = BuildSampleEndpointRef(url, deployed.DeploymentId, entry.Sample.Id),
            Workload = BuildSampleWorkload(),
            MaxDurationSecs = 900,
            CreatedBy = userId,
            CreatedAt = now,
            UpdatedAt = now,
        };

        // The token that reaches the sample is the one the deploy staged: copy
        // the ciphertext over verbatim so the registration can authenticate
        // without the plaintext ever existing outside the cipher.
        var deployment = await db.Deployments
            .AsNoTracking()
            .Where(d => d.DeploymentId == deployed.DeploymentId)
            .Select(d => d.Config)
            .FirstOrDefaultAsync(ct)
            .ConfigureAwait(false);
        var stored = ReadTokenCipher(deployment);
        if (stored is not null)
        {
            cfg.TokenEnc = stored.Value.Enc;
            cfg.TokenNonce = stored.Value.Nonce;
        }

        db.TestConfigs.Add(cfg);
        try
        {
            await db.SaveChangesAsync(ct).ConfigureAwait(false);
        }
        catch (DbUpdateException)
        {
            // Unique (project, name) collision — the caller reports it.
            db.Entry(cfg).State = EntityState.Detached;
            return null;
        }
        return cfg;
    }

    /// <summary>The endpoint_ref of a sample registration: the plain network
    /// target plus the breadcrumb tying it back to the deployment + language,
    /// which is what makes "already registered" answerable without a new
    /// column.</summary>
    internal static string BuildSampleEndpointRef(string url, Guid deploymentId, string language) =>
        new JsonObject
        {
            ["kind"] = "network",
            ["host"] = url,
            ["deployment_id"] = deploymentId.ToString(),
            ["sdk_sample"] = language,
        }.ToJsonString();

    private static string BuildSampleWorkload() =>
        new JsonObject
        {
            ["modes"] = new JsonArray(SdkEndpointsEndpoints.SdkProbeMode),
            ["runs"] = 10,
            ["concurrency"] = 1,
            ["timeout_ms"] = 30_000,
            ["laghound_route"] = SdkSampleCatalog.RouteDefault,
        }.ToJsonString();

    /// <summary>Read the (deployment, language) breadcrumb off an endpoint_ref;
    /// null when the config is not a sample registration.</summary>
    internal static (Guid DeploymentId, string Language)? ReadSampleLink(string? endpointRef)
    {
        if (string.IsNullOrWhiteSpace(endpointRef))
        {
            return null;
        }
        try
        {
            using var doc = JsonDocument.Parse(endpointRef);
            if (doc.RootElement.ValueKind != JsonValueKind.Object
                || !doc.RootElement.TryGetProperty("deployment_id", out var d)
                || d.ValueKind != JsonValueKind.String
                || !Guid.TryParse(d.GetString(), out var id)
                || !doc.RootElement.TryGetProperty("sdk_sample", out var l)
                || l.ValueKind != JsonValueKind.String
                || SdkSampleCatalog.Find(l.GetString()) is not { } sample)
            {
                return null;
            }
            return (id, sample.Id);
        }
        catch (JsonException)
        {
            return null;
        }
    }

    private static string LinkKey(Guid deploymentId, string language) => $"{deploymentId:N}:{language}";

    // ── Token handling ───────────────────────────────────────────────────────

    /// <summary>A fresh sample token: 32 URL-safe bytes, comfortably over the
    /// contract's 16-byte floor.</summary>
    internal static string MintToken() =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(24))
            .Replace('+', '-').Replace('/', '_').TrimEnd('=');

    /// <summary>Read the AES-GCM ciphertext the deploy config carries, if any.</summary>
    internal static (byte[] Enc, byte[] Nonce)? ReadTokenCipher(string? configJson)
    {
        if (string.IsNullOrWhiteSpace(configJson))
        {
            return null;
        }
        try
        {
            var root = JsonNode.Parse(configJson);
            if (root?[TokenConfigKey] is not JsonObject tok)
            {
                return null;
            }
            var enc = tok["token_enc"]?.GetValue<string>();
            var nonce = tok["token_nonce"]?.GetValue<string>();
            if (string.IsNullOrWhiteSpace(enc) || string.IsNullOrWhiteSpace(nonce))
            {
                return null;
            }
            return (Convert.FromBase64String(enc), Convert.FromBase64String(nonce));
        }
        catch (Exception ex) when (ex is JsonException or FormatException or InvalidOperationException)
        {
            return null;
        }
    }

    /// <summary>
    /// The plaintext token that can read a sample's health: the deployment's
    /// staged ciphertext first, else the token stored on an existing SDK
    /// endpoint registration for that deployment+language. Null when neither
    /// exists (the probe then reports "up, version unreadable").
    /// </summary>
    private static string? ResolveToken(
        CredentialCipher cipher,
        string? deploymentConfig,
        IReadOnlyList<Data.Entities.TestConfig> configs,
        Guid deploymentId,
        string language)
    {
        var staged = ReadTokenCipher(deploymentConfig);
        if (staged is not null && TryDecrypt(cipher, staged.Value.Enc, staged.Value.Nonce, out var fromConfig))
        {
            return fromConfig;
        }
        foreach (var cfg in configs)
        {
            if (cfg.TokenEnc is not { Length: > 0 } enc || cfg.TokenNonce is not { Length: > 0 } nonce)
            {
                continue;
            }
            var link = ReadSampleLink(cfg.EndpointRef);
            if (link?.DeploymentId == deploymentId && link.Value.Language == language
                && TryDecrypt(cipher, enc, nonce, out var fromConfigRow))
            {
                return fromConfigRow;
            }
        }
        return null;
    }

    private static bool TryDecrypt(CredentialCipher cipher, byte[] enc, byte[] nonce, out string plaintext)
    {
        try
        {
            plaintext = Encoding.UTF8.GetString(cipher.Decrypt(enc, nonce));
            return true;
        }
        catch (Exception ex) when (ex is CryptographicException or ArgumentException)
        {
            // A key rotation this cipher cannot follow — treat as no token.
            plaintext = string.Empty;
            return false;
        }
    }

    // ── Deploy-config construction ───────────────────────────────────────────

    /// <summary>Where a create should provision (validated form of the request).</summary>
    internal sealed record Target(
        SdkSamplePlan.Shape Shape,
        string Provider,
        string? Region,
        string? VmSize,
        Guid? CloudAccountId);

    private static bool TryResolveTarget(CreateSamplesRequest req, out Target target, out string? error)
    {
        target = default!;
        error = null;
        var shape = SdkSamplePlan.ParseShape(req.Shape)!.Value;
        var provider = req.Provider?.Trim().ToLowerInvariant();
        if (string.IsNullOrWhiteSpace(provider))
        {
            error = "provider is required to provision a sample server (azure, aws, gcp or docker)";
            return false;
        }
        if (provider is not ("azure" or "aws" or "gcp" or "docker"))
        {
            error = $"unsupported provider '{provider}' — samples deploy to azure, aws, gcp or docker";
            return false;
        }
        if (provider != "docker")
        {
            if (string.IsNullOrWhiteSpace(req.Region))
            {
                error = "region is required for a cloud sample server";
                return false;
            }
            if (string.IsNullOrWhiteSpace(req.VmSize))
            {
                error = "vm_size is required for a cloud sample server";
                return false;
            }
        }
        Guid? accountId = null;
        if (!string.IsNullOrWhiteSpace(req.CloudAccountId))
        {
            if (!Guid.TryParse(req.CloudAccountId, out var parsed))
            {
                error = "cloud_account_id must be a uuid";
                return false;
            }
            accountId = parsed;
        }
        target = new Target(
            shape,
            provider,
            provider == "docker" ? "local" : req.Region!.Trim(),
            provider == "docker" ? "container" : req.VmSize!.Trim(),
            accountId);
        return true;
    }

    /// <summary>
    /// How the languages that must be provisioned split into DEPLOYMENTS.
    /// Consolidated ⇒ one deployment carrying them all; separated ⇒ one per
    /// language. (How each deployment then splits into servers is
    /// <see cref="BuildDeployConfig"/>'s job — the docker provider always gets
    /// one container per sample because a sample image runs exactly one
    /// sample.)
    /// </summary>
    internal static List<List<string>> GroupForShape(Target target, IReadOnlyList<string> provision) =>
        target.Shape == SdkSamplePlan.Shape.Consolidated
            ? [provision.ToList()]
            : provision.Select(l => new List<string> { l }).ToList();

    /// <summary>
    /// The deploy.json for one sample deployment. Deliberately minimal: no
    /// proxy stacks, no reference-API languages, tests off — the endpoint
    /// exists to serve the samples, and every extra install is money and
    /// failure surface. Pure so the shape is unit-testable
    /// (SdkSampleConfigTests).
    ///
    /// <para>Cloud providers get ONE endpoint carrying every language (the
    /// consolidated promise: one VM, one bill, one sample process per catalog
    /// port). The docker provider gets one endpoint PER language, because a
    /// sample image runs exactly one sample — so "consolidated" there means one
    /// deployment, not one container. That difference is free (containers cost
    /// nothing) and keeps <c>endpoint_ips[i]</c> parallel to the endpoints,
    /// which is what discovery reads.</para>
    /// </summary>
    internal static JsonObject BuildDeployConfig(Target target, IReadOnlyList<string> languages, JsonObject tokenNode)
    {
        var perEndpoint = target.Provider == "docker"
            ? languages.Select(l => new List<string> { l }).ToList()
            : [languages.ToList()];

        var endpoints = new JsonArray();
        foreach (var group in perEndpoint)
        {
            var samples = new JsonArray();
            foreach (var l in group)
            {
                samples.Add(l);
            }

            var endpoint = new JsonObject
            {
                ["provider"] = target.Provider,
                ["label"] = group.Count == 1 ? $"sdk-{group[0]}" : "sdk-samples",
                [ConfigKey] = samples,
            };

            var suffix = Guid.NewGuid().ToString("N")[..4];
            switch (target.Provider)
            {
                case "azure":
                    endpoint["azure"] = new JsonObject
                    {
                        ["region"] = target.Region,
                        ["vm_size"] = target.VmSize,
                        ["os"] = SampleOs,
                        ["vm_name"] = $"nwk-sdk-{suffix}",
                    };
                    break;
                case "aws":
                    endpoint["aws"] = new JsonObject
                    {
                        ["region"] = target.Region,
                        ["instance_type"] = target.VmSize,
                        ["os"] = SampleOs,
                        ["instance_name"] = $"nwk-sdk-{suffix}",
                    };
                    break;
                case "gcp":
                    endpoint["gcp"] = new JsonObject
                    {
                        ["region"] = target.Region,
                        ["zone"] = $"{target.Region}-a",
                        ["machine_type"] = target.VmSize,
                        ["os"] = SampleOs,
                        ["instance_name"] = $"nwk-sdk-{suffix}",
                    };
                    break;
                default:
                    endpoint["docker"] = new JsonObject { ["os"] = SampleOs };
                    break;
            }
            endpoints.Add(endpoint);
        }

        var config = new JsonObject
        {
            ["version"] = 1,
            ["tester"] = new JsonObject { ["provider"] = "local" },
            ["endpoints"] = endpoints,
            [TokenConfigKey] = tokenNode.DeepClone(),
            ["tests"] = new JsonObject { ["run_tests"] = false },
        };
        if (target.CloudAccountId is { } accountId && target.Provider != "docker")
        {
            config["cloud_account_id"] = accountId.ToString();
        }
        return config;
    }

    /// <summary>Deployment name for a sample server — languages plus a short
    /// suffix, because repeat deploys otherwise collide on name.</summary>
    internal static string DeploymentName(IReadOnlyList<string> languages) =>
        $"sdk-{string.Join("-", languages)}-{Guid.NewGuid().ToString("N")[..4]}";

    // ── Cost preview ─────────────────────────────────────────────────────────

    /// <summary>
    /// What one sample server costs, so the UI can put a number on
    /// "consolidated is cheaper". Priced by the SAME
    /// <see cref="CostEstimation"/> helpers the tester and deployment cost
    /// endpoints use — there is exactly one price table in this repo. Returns
    /// null when the caller did not name a provider/size to price.
    /// </summary>
    internal static async Task<object?> BuildCostPreviewAsync(
        NetworkerDbContext db, string? provider, string? region, string? vmSize, CancellationToken ct)
    {
        var p = provider?.Trim().ToLowerInvariant();
        if (string.IsNullOrWhiteSpace(p) || string.IsNullOrWhiteSpace(vmSize))
        {
            return null;
        }
        if (p == "docker")
        {
            return new
            {
                provider = p,
                region = "local",
                vm_size = "container",
                hourly_usd = 0.0,
                monthly_usd = 0.0,
                note = "containers on the control-plane host — no cloud spend",
            };
        }
        var hourly = await CostEstimation.HourlyUsdAsync(db, p, vmSize.Trim(), region?.Trim()).ConfigureAwait(false);
        return new
        {
            provider = p,
            region = region?.Trim(),
            vm_size = vmSize.Trim(),
            hourly_usd = hourly,
            monthly_usd = 24.0 * 30.0 * hourly,
            note = "per server, always-on (deploy VMs carry no auto-shutdown schedule)",
        };
    }

    // ── Small helpers ────────────────────────────────────────────────────────

    internal static string SampleUrl(string host, int port) => $"http://{host}:{port}";

    private static string? At(IReadOnlyList<string?> list, int index) =>
        index >= 0 && index < list.Count ? list[index] : null;

    private static List<string?> ParseStringArray(string? raw)
    {
        var list = new List<string?>();
        if (string.IsNullOrWhiteSpace(raw))
        {
            return list;
        }
        try
        {
            using var doc = JsonDocument.Parse(raw);
            if (doc.RootElement.ValueKind == JsonValueKind.Array)
            {
                foreach (var el in doc.RootElement.EnumerateArray())
                {
                    list.Add(el.ValueKind == JsonValueKind.String ? el.GetString() : null);
                }
            }
        }
        catch (JsonException)
        {
            // malformed column — treat as no entries
        }
        return list;
    }

    // ── Request body (snake_case JSON) ───────────────────────────────────────

    /// <summary>Create body: which languages, in which shape, on what.</summary>
    public sealed record CreateSamplesRequest(
        [property: JsonPropertyName("shape")] string? Shape,
        [property: JsonPropertyName("languages")] IReadOnlyList<string>? Languages,
        [property: JsonPropertyName("reuse_existing")] bool? ReuseExisting,
        [property: JsonPropertyName("provider")] string? Provider,
        [property: JsonPropertyName("region")] string? Region,
        [property: JsonPropertyName("vm_size")] string? VmSize,
        [property: JsonPropertyName("cloud_account_id")] string? CloudAccountId);
}
