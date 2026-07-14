using Microsoft.AspNetCore.Authentication.JwtBearer;
using Microsoft.AspNetCore.Authorization;
using Npgsql;

namespace Networker.ControlPlane.Auth;

/// <summary>
/// Wires the auth + RBAC foundation into the ControlPlane. Program.cs calls
/// <see cref="AddNetworkerAuth"/> during service registration and
/// <see cref="MapAuthEndpoints"/> / <see cref="UseNetworkerAuth"/> in the pipeline.
///
/// Everything here is additive: authentication runs, the /auth/* endpoints mount,
/// and the policies become available — but no existing route is forced to require
/// auth. Endpoints opt in with <c>.RequireAuthorization(AuthPolicies.*)</c>.
/// </summary>
public static class AuthExtensions
{
    public static IServiceCollection AddNetworkerAuth(
        this IServiceCollection services,
        string npgsqlConnectionString)
    {
        // Raw-SQL data source (EF-free) from the same connection string the
        // ControlPlane already resolves — keeps this work decoupled from the
        // parallel EF-model changes.
        var dataSource = new NpgsqlDataSourceBuilder(npgsqlConnectionString).Build();
        services.AddSingleton(dataSource);
        services.AddScoped<AuthRepository>();

        var secret = Environment.GetEnvironmentVariable(JwtTokenService.SecretEnvVar) ?? string.Empty;
        // Dev fallback so the app still boots without the env var; the Rust side
        // hard-requires it. Any real deployment sets DASHBOARD_JWT_SECRET and the
        // two implementations then share the exact same signing key.
        if (string.IsNullOrEmpty(secret))
        {
            secret = "dev-insecure-jwt-secret-change-me-please-32b";
        }

        var tokenService = new JwtTokenService(secret);
        services.AddSingleton(tokenService);

        services.AddHttpContextAccessor();
        services.AddMemoryCache();
        services.AddScoped<AuthUserAccessor>();

        services.AddSingleton<IAuthorizationHandler, GlobalRoleHandler>();
        services.AddScoped<IAuthorizationHandler, ProjectRoleHandler>();

        services
            .AddAuthentication(JwtBearerDefaults.AuthenticationScheme)
            .AddJwtBearer(options =>
            {
                options.TokenValidationParameters = tokenService.ValidationParameters;
                // Keep sub/email/role claim names verbatim (no legacy remapping),
                // so AuthUser.FromPrincipal reads the same names Rust emits.
                options.MapInboundClaims = false;
            });

        services.AddAuthorizationBuilder()
            .AddPolicy(AuthPolicies.GlobalAdmin, p =>
                p.AddRequirements(new GlobalRoleRequirement(Role.Admin)))
            .AddPolicy(AuthPolicies.GlobalOperator, p =>
                p.AddRequirements(new GlobalRoleRequirement(Role.Operator)))
            .AddPolicy(AuthPolicies.GlobalViewer, p =>
                p.AddRequirements(new GlobalRoleRequirement(Role.Viewer)))
            .AddPolicy(AuthPolicies.ProjectMember, p =>
                p.AddRequirements(new ProjectRoleRequirement(ProjectRole.Viewer)))
            .AddPolicy(AuthPolicies.ProjectOperator, p =>
                p.AddRequirements(new ProjectRoleRequirement(ProjectRole.Operator)))
            .AddPolicy(AuthPolicies.ProjectAdmin, p =>
                p.AddRequirements(new ProjectRoleRequirement(ProjectRole.Admin)));

        return services;
    }

    /// <summary>
    /// Insert authn, authz, and the DB-status middleware into the pipeline.
    /// Must run after routing so route values ({projectId}) are available to the
    /// project-scope handler, and the status middleware runs after authentication
    /// so it can read the validated principal.
    /// </summary>
    public static WebApplication UseNetworkerAuth(this WebApplication app)
    {
        app.UseAuthentication();
        app.UseMiddleware<UserStatusMiddleware>();
        app.UseAuthorization();
        return app;
    }

    /// <summary>
    /// Map the /auth/* endpoints, matching the Rust dashboard's response shapes.
    /// login is anonymous; profile requires a valid token.
    /// </summary>
    public static WebApplication MapAuthEndpoints(this WebApplication app)
    {
        // POST /auth/login — email+password → bcrypt verify → mint JWT.
        app.MapPost("/auth/login", async (
            LoginRequest req,
            AuthRepository repo,
            JwtTokenService tokens,
            CancellationToken ct) =>
        {
            var candidate = await repo.FindByEmailForLoginAsync(req.Email, ct);
            if (candidate is null)
            {
                return Results.Unauthorized();
            }

            // Rust: only 'active' local accounts with a password may log in.
            if (candidate.Status != "active" || candidate.SsoOnly || candidate.PasswordHash is null)
            {
                return Results.Unauthorized();
            }

            bool valid;
            try
            {
                valid = BCrypt.Net.BCrypt.Verify(req.Password, candidate.PasswordHash);
            }
            catch (Exception)
            {
                // Malformed hash → treat as invalid credentials, never 500.
                valid = false;
            }

            if (!valid)
            {
                return Results.Unauthorized();
            }

            await repo.TouchLastLoginAsync(candidate.UserId, ct);

            var token = tokens.CreateToken(
                candidate.UserId, candidate.Email, candidate.Role, candidate.IsPlatformAdmin);

            return Results.Ok(new LoginResponse(
                token,
                candidate.Role,
                candidate.Email,
                candidate.Status,
                candidate.MustChangePassword));
        });

        // GET /auth/profile — current user (email, role, status).
        app.MapGet("/auth/profile", async (
            HttpContext ctx,
            AuthRepository repo,
            CancellationToken ct) =>
        {
            var user = ctx.GetAuthUser();
            if (user is null)
            {
                return Results.Unauthorized();
            }

            var profile = await repo.GetProfileAsync(user.UserId, ct);
            var email = profile?.Email ?? user.Email;
            var status = profile?.Status ?? "active";

            return Results.Ok(new ProfileResponse(email, user.Role, status));
        }).RequireAuthorization(AuthPolicies.GlobalViewer);

        return app;
    }
}

/// <summary>POST /auth/login body — matches Rust <c>LoginRequest</c>.</summary>
public sealed record LoginRequest(string Email, string Password);

/// <summary>POST /auth/login response — matches Rust <c>LoginResponse</c> field-for-field.</summary>
public sealed record LoginResponse(
    string token,
    string role,
    string email,
    string status,
    bool must_change_password);

/// <summary>GET /auth/profile response — matches the Rust JSON { email, role, status }.</summary>
public sealed record ProfileResponse(string email, string role, string status);
