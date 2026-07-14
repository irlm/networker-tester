using System;
using System.Collections.Generic;
using Microsoft.EntityFrameworkCore;
using Networker.Data.Entities;
using Npgsql.EntityFrameworkCore.PostgreSQL.Metadata;

namespace Networker.Data;

public partial class NetworkerDbContext : DbContext
{
    public NetworkerDbContext(DbContextOptions<NetworkerDbContext> options)
        : base(options)
    {
    }

    public virtual DbSet<Agent> Agents { get; set; }

    public virtual DbSet<CloudAccount> CloudAccounts { get; set; }

    public virtual DbSet<Project> Projects { get; set; }

    public virtual DbSet<ProjectTester> ProjectTesters { get; set; }

    public virtual DbSet<TestConfig> TestConfigs { get; set; }

    public virtual DbSet<TestRun> TestRuns { get; set; }

    public virtual DbSet<DashUser> DashUsers { get; set; }

    public virtual DbSet<Deployment> Deployments { get; set; }

    public virtual DbSet<CloudConnection> CloudConnections { get; set; }

    public virtual DbSet<ProjectMember> ProjectMembers { get; set; }

    public virtual DbSet<ShareLink> ShareLinks { get; set; }

    public virtual DbSet<CommandApproval> CommandApprovals { get; set; }

    public virtual DbSet<TestVisibilityRule> TestVisibilityRules { get; set; }

    public virtual DbSet<WorkspaceInvite> WorkspaceInvites { get; set; }

    public virtual DbSet<WorkspaceWarning> WorkspaceWarnings { get; set; }

    public virtual DbSet<BenchmarkVmCatalog> BenchmarkVmCatalogs { get; set; }

    public virtual DbSet<SovereigntyZone> SovereigntyZones { get; set; }

    public virtual DbSet<ServerRegistry> ServerRegistries { get; set; }

    public virtual DbSet<ProjectRouting> ProjectRoutings { get; set; }

    public virtual DbSet<MigrationRequest> MigrationRequests { get; set; }

    public virtual DbSet<MigrationAuditLog> MigrationAuditLogs { get; set; }

    public virtual DbSet<SystemHealth> SystemHealths { get; set; }

    public virtual DbSet<SsoProvider> SsoProviders { get; set; }

    public virtual DbSet<SystemConfig> SystemConfigs { get; set; }

    public virtual DbSet<AgentCommand> AgentCommands { get; set; }

    public virtual DbSet<VmLifecycle> VmLifecycles { get; set; }

    public virtual DbSet<CostRate> CostRates { get; set; }

    public virtual DbSet<BenchmarkArtifact> BenchmarkArtifacts { get; set; }

    public virtual DbSet<TestSchedule> TestSchedules { get; set; }

    public virtual DbSet<ComparisonGroup> ComparisonGroups { get; set; }

    protected override void OnModelCreating(ModelBuilder modelBuilder)
    {
        modelBuilder
            .HasPostgresExtension("timescaledb")
            .HasPostgresExtension("timescaledb_toolkit");

        modelBuilder.Entity<Agent>(entity =>
        {
            entity.HasKey(e => e.AgentId).HasName("agent_pkey");

            entity.ToTable("agent");

            entity.HasIndex(e => e.ApiKey, "agent_api_key_key").IsUnique();

            entity.HasIndex(e => e.TesterId, "idx_agent_tester").HasFilter("(tester_id IS NOT NULL)");

            entity.HasIndex(e => e.ProjectId, "ix_agent_project");

            entity.HasIndex(e => e.Status, "ix_agent_status");

            entity.Property(e => e.AgentId)
                .ValueGeneratedNever()
                .HasColumnName("agent_id");
            entity.Property(e => e.ApiKey)
                .HasMaxLength(255)
                .HasColumnName("api_key");
            entity.Property(e => e.Arch)
                .HasMaxLength(20)
                .HasColumnName("arch");
            entity.Property(e => e.LastHeartbeat).HasColumnName("last_heartbeat");
            entity.Property(e => e.Name)
                .HasMaxLength(100)
                .HasColumnName("name");
            entity.Property(e => e.Os)
                .HasMaxLength(50)
                .HasColumnName("os");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Provider)
                .HasMaxLength(20)
                .HasColumnName("provider");
            entity.Property(e => e.Region)
                .HasMaxLength(100)
                .HasColumnName("region");
            entity.Property(e => e.RegisteredAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("registered_at");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'offline'::character varying")
                .HasColumnName("status");
            entity.Property(e => e.Tags)
                .HasColumnType("jsonb")
                .HasColumnName("tags");
            entity.Property(e => e.TesterId).HasColumnName("tester_id");
            entity.Property(e => e.Version)
                .HasMaxLength(50)
                .HasColumnName("version");

            entity.HasOne(d => d.Project).WithMany(p => p.Agents)
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.ClientSetNull)
                .HasConstraintName("agent_project_id_fkey");

            entity.HasOne(d => d.Tester).WithMany(p => p.Agents)
                .HasForeignKey(d => d.TesterId)
                .OnDelete(DeleteBehavior.SetNull)
                .HasConstraintName("agent_tester_id_fkey");
        });

        modelBuilder.Entity<CloudAccount>(entity =>
        {
            entity.HasKey(e => e.AccountId).HasName("cloud_account_pkey");

            entity.ToTable("cloud_account");

            entity.HasIndex(e => e.OwnerId, "ix_cloud_account_owner").HasFilter("(owner_id IS NOT NULL)");

            entity.HasIndex(e => e.ProjectId, "ix_cloud_account_project");

            entity.Property(e => e.AccountId)
                .ValueGeneratedNever()
                .HasColumnName("account_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CredentialsEnc).HasColumnName("credentials_enc");
            entity.Property(e => e.CredentialsNonce).HasColumnName("credentials_nonce");
            entity.Property(e => e.LastValidated).HasColumnName("last_validated");
            entity.Property(e => e.Name)
                .HasMaxLength(200)
                .HasColumnName("name");
            entity.Property(e => e.OwnerId).HasColumnName("owner_id");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Provider)
                .HasMaxLength(20)
                .HasColumnName("provider");
            entity.Property(e => e.RegionDefault)
                .HasMaxLength(100)
                .HasColumnName("region_default");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'active'::character varying")
                .HasColumnName("status");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
            entity.Property(e => e.ValidationError).HasColumnName("validation_error");

            entity.HasOne(d => d.Project).WithMany(p => p.CloudAccounts)
                .HasForeignKey(d => d.ProjectId)
                .HasConstraintName("cloud_account_project_id_fkey");
        });

        modelBuilder.Entity<Project>(entity =>
        {
            entity.HasKey(e => e.ProjectId).HasName("project_pkey");

            entity.ToTable("project");

            entity.HasIndex(e => e.Slug, "ix_project_slug");

            entity.HasIndex(e => e.Slug, "project_slug_key").IsUnique();

            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.DeleteProtection)
                .HasDefaultValue(false)
                .HasColumnName("delete_protection");
            entity.Property(e => e.DeletedAt).HasColumnName("deleted_at");
            entity.Property(e => e.Description).HasColumnName("description");
            entity.Property(e => e.Name)
                .HasMaxLength(200)
                .HasColumnName("name");
            entity.Property(e => e.Settings)
                .HasDefaultValueSql("'{}'::jsonb")
                .HasColumnType("jsonb")
                .HasColumnName("settings");
            entity.Property(e => e.Slug)
                .HasMaxLength(100)
                .HasColumnName("slug");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
        });

        modelBuilder.Entity<ProjectTester>(entity =>
        {
            entity.HasKey(e => e.TesterId).HasName("project_tester_pkey");

            entity.ToTable("project_tester");

            entity.HasIndex(e => e.Allocation, "idx_project_tester_alloc").HasFilter("(allocation = ANY (ARRAY['idle'::text, 'locked'::text]))");

            entity.HasIndex(e => e.CloudConnectionId, "idx_project_tester_cloud_conn").HasFilter("(cloud_connection_id IS NOT NULL)");

            entity.HasIndex(e => new { e.ProjectId, e.LastUsedAt }, "idx_project_tester_last_used")
                .IsDescending(false, true)
                .HasNullSortOrder(new[] { NullSortOrder.NullsLast, NullSortOrder.NullsLast });

            entity.HasIndex(e => e.PowerState, "idx_project_tester_power").HasFilter("(power_state = ANY (ARRAY['running'::text, 'stopped'::text]))");

            entity.HasIndex(e => e.ProjectId, "idx_project_tester_project");

            entity.HasIndex(e => e.NextShutdownAt, "idx_project_tester_shutdown").HasFilter("(auto_shutdown_enabled = true)");

            entity.HasIndex(e => new { e.ProjectId, e.Name }, "project_tester_project_id_name_key").IsUnique();

            entity.Property(e => e.TesterId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("tester_id");
            entity.Property(e => e.Allocation)
                .HasDefaultValueSql("'idle'::text")
                .HasColumnName("allocation");
            entity.Property(e => e.AutoProbeEnabled)
                .HasDefaultValue(false)
                .HasColumnName("auto_probe_enabled");
            entity.Property(e => e.AutoShutdownEnabled)
                .HasDefaultValue(true)
                .HasColumnName("auto_shutdown_enabled");
            entity.Property(e => e.AutoShutdownLocalHour)
                .HasDefaultValue((short)23)
                .HasColumnName("auto_shutdown_local_hour");
            entity.Property(e => e.AvgBenchmarkDurationSeconds).HasColumnName("avg_benchmark_duration_seconds");
            entity.Property(e => e.BenchmarkRunCount)
                .HasDefaultValue(0)
                .HasColumnName("benchmark_run_count");
            entity.Property(e => e.Cloud).HasColumnName("cloud");
            entity.Property(e => e.CloudAccountId).HasColumnName("cloud_account_id");
            entity.Property(e => e.CloudConnectionId).HasColumnName("cloud_connection_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.InstallerVersion).HasColumnName("installer_version");
            entity.Property(e => e.LastInstalledAt).HasColumnName("last_installed_at");
            entity.Property(e => e.LastUsedAt).HasColumnName("last_used_at");
            entity.Property(e => e.LockedByConfigId).HasColumnName("locked_by_config_id");
            entity.Property(e => e.Name).HasColumnName("name");
            entity.Property(e => e.NextShutdownAt).HasColumnName("next_shutdown_at");
            entity.Property(e => e.OsArch)
                .HasMaxLength(20)
                .HasColumnName("os_arch");
            entity.Property(e => e.OsDistro)
                .HasMaxLength(50)
                .HasColumnName("os_distro");
            entity.Property(e => e.OsKernel)
                .HasMaxLength(100)
                .HasColumnName("os_kernel");
            entity.Property(e => e.OsVariant)
                .HasMaxLength(20)
                .HasColumnName("os_variant");
            entity.Property(e => e.OsVersion)
                .HasMaxLength(50)
                .HasColumnName("os_version");
            entity.Property(e => e.PowerState)
                .HasDefaultValueSql("'provisioning'::text")
                .HasColumnName("power_state");
            entity.Property(e => e.ProjectId).HasColumnName("project_id");
            entity.Property(e => e.PublicIp).HasColumnName("public_ip");
            entity.Property(e => e.Region).HasColumnName("region");
            entity.Property(e => e.RequestedOs)
                .HasMaxLength(50)
                .HasDefaultValueSql("'ubuntu-24.04'::character varying")
                .HasColumnName("requested_os");
            entity.Property(e => e.RequestedVariant)
                .HasMaxLength(20)
                .HasDefaultValueSql("'server'::character varying")
                .HasColumnName("requested_variant");
            entity.Property(e => e.ShutdownDeferralCount)
                .HasDefaultValue((short)0)
                .HasColumnName("shutdown_deferral_count");
            entity.Property(e => e.SshUser)
                .HasDefaultValueSql("'azureuser'::text")
                .HasColumnName("ssh_user");
            entity.Property(e => e.StatusMessage).HasColumnName("status_message");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
            entity.Property(e => e.VmName).HasColumnName("vm_name");
            entity.Property(e => e.VmResourceId).HasColumnName("vm_resource_id");
            entity.Property(e => e.VmSize)
                .HasDefaultValueSql("'Standard_D2s_v3'::text")
                .HasColumnName("vm_size");

            entity.HasOne(d => d.CloudAccount).WithMany(p => p.ProjectTesters)
                .HasForeignKey(d => d.CloudAccountId)
                .OnDelete(DeleteBehavior.SetNull)
                .HasConstraintName("project_tester_cloud_account_id_fkey");

            entity.HasOne(d => d.Project).WithMany(p => p.ProjectTesters)
                .HasForeignKey(d => d.ProjectId)
                .HasConstraintName("project_tester_project_id_fkey");
        });

        modelBuilder.Entity<TestConfig>(entity =>
        {
            entity.HasKey(e => e.Id).HasName("test_config_pkey");

            entity.ToTable("test_config");

            entity.HasIndex(e => e.EndpointKind, "ix_test_config_endpoint_kind");

            entity.HasIndex(e => e.ProjectId, "ix_test_config_project");

            entity.HasIndex(e => new { e.ProjectId, e.Name }, "test_config_project_id_name_key").IsUnique();

            entity.Property(e => e.Id)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("id");
            entity.Property(e => e.BaselineRunId).HasColumnName("baseline_run_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.Description).HasColumnName("description");
            entity.Property(e => e.EndpointKind).HasColumnName("endpoint_kind");
            entity.Property(e => e.EndpointRef)
                .HasColumnType("jsonb")
                .HasColumnName("endpoint_ref");
            entity.Property(e => e.MaxDurationSecs)
                .HasDefaultValue(900)
                .HasColumnName("max_duration_secs");
            entity.Property(e => e.Methodology)
                .HasColumnType("jsonb")
                .HasColumnName("methodology");
            entity.Property(e => e.Name).HasColumnName("name");
            entity.Property(e => e.ProjectId).HasColumnName("project_id");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
            entity.Property(e => e.Workload)
                .HasColumnType("jsonb")
                .HasColumnName("workload");

            entity.HasOne(d => d.BaselineRun).WithMany(p => p.TestConfigs)
                .HasForeignKey(d => d.BaselineRunId)
                .OnDelete(DeleteBehavior.SetNull)
                .HasConstraintName("test_config_baseline_run_fk");

            entity.HasOne(d => d.Project).WithMany(p => p.TestConfigs)
                .HasForeignKey(d => d.ProjectId)
                .HasConstraintName("test_config_project_id_fkey");
        });

        modelBuilder.Entity<TestRun>(entity =>
        {
            entity.HasKey(e => e.Id).HasName("test_run_pkey");

            entity.ToTable("test_run");

            entity.HasIndex(e => e.ComparisonGroupId, "ix_test_run_comparison").HasFilter("(comparison_group_id IS NOT NULL)");

            entity.HasIndex(e => e.TestConfigId, "ix_test_run_config");

            entity.HasIndex(e => e.CreatedAt, "ix_test_run_created").IsDescending();

            entity.HasIndex(e => new { e.ProjectId, e.Status }, "ix_test_run_project_status");

            entity.HasIndex(e => e.ProvisioningDeploymentId, "ix_test_run_provisioning").HasFilter("(provisioning_deployment_id IS NOT NULL)");

            entity.Property(e => e.Id)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("id");
            entity.Property(e => e.ArtifactId).HasColumnName("artifact_id");
            entity.Property(e => e.ComparisonGroupId).HasColumnName("comparison_group_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.ErrorMessage).HasColumnName("error_message");
            entity.Property(e => e.FailureCount)
                .HasDefaultValue(0)
                .HasColumnName("failure_count");
            entity.Property(e => e.FinishedAt).HasColumnName("finished_at");
            entity.Property(e => e.LastHeartbeat).HasColumnName("last_heartbeat");
            entity.Property(e => e.ProjectId).HasColumnName("project_id");
            entity.Property(e => e.ProvisioningDeploymentId).HasColumnName("provisioning_deployment_id");
            entity.Property(e => e.StartedAt).HasColumnName("started_at");
            entity.Property(e => e.Status).HasColumnName("status");
            entity.Property(e => e.SuccessCount)
                .HasDefaultValue(0)
                .HasColumnName("success_count");
            entity.Property(e => e.TestConfigId).HasColumnName("test_config_id");
            entity.Property(e => e.TesterId).HasColumnName("tester_id");
            entity.Property(e => e.WorkerId).HasColumnName("worker_id");

            entity.HasOne(d => d.Project).WithMany(p => p.TestRuns)
                .HasForeignKey(d => d.ProjectId)
                .HasConstraintName("test_run_project_id_fkey");

            entity.HasOne(d => d.TestConfig).WithMany(p => p.TestRuns)
                .HasForeignKey(d => d.TestConfigId)
                .HasConstraintName("test_run_test_config_id_fkey");

            entity.HasOne(d => d.Tester).WithMany(p => p.TestRuns)
                .HasForeignKey(d => d.TesterId)
                .OnDelete(DeleteBehavior.SetNull)
                .HasConstraintName("test_run_tester_id_fkey");
        });
        modelBuilder.Entity<DashUser>(entity =>
        {
            entity.HasKey(e => e.UserId).HasName("dash_user_pkey");

            entity.ToTable("dash_user");

            entity.HasIndex(e => e.Email, "dash_user_email_unique").IsUnique();

            entity.Property(e => e.UserId)
                .ValueGeneratedNever()
                .HasColumnName("user_id");
            entity.Property(e => e.AuthProvider)
                .HasMaxLength(20)
                .HasDefaultValueSql("'local'::character varying")
                .HasColumnName("auth_provider");
            entity.Property(e => e.AvatarUrl).HasColumnName("avatar_url");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.DisplayName)
                .HasMaxLength(200)
                .HasColumnName("display_name");
            entity.Property(e => e.Email)
                .HasMaxLength(255)
                .HasColumnName("email");
            entity.Property(e => e.IsPlatformAdmin)
                .HasDefaultValue(false)
                .HasColumnName("is_platform_admin");
            entity.Property(e => e.LastLoginAt).HasColumnName("last_login_at");
            entity.Property(e => e.MustChangePassword)
                .HasDefaultValue(false)
                .HasColumnName("must_change_password");
            entity.Property(e => e.PasswordHash)
                .HasMaxLength(255)
                .HasColumnName("password_hash");
            entity.Property(e => e.PasswordResetExpires).HasColumnName("password_reset_expires");
            entity.Property(e => e.PasswordResetToken)
                .HasMaxLength(128)
                .HasColumnName("password_reset_token");
            entity.Property(e => e.Role)
                .HasMaxLength(20)
                .HasDefaultValueSql("'viewer'::character varying")
                .HasColumnName("role");
            entity.Property(e => e.SsoOnly)
                .HasDefaultValue(false)
                .HasColumnName("sso_only");
            entity.Property(e => e.SsoSubjectId)
                .HasMaxLength(255)
                .HasColumnName("sso_subject_id");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'pending'::character varying")
                .HasColumnName("status");
        });

        modelBuilder.Entity<Deployment>(entity =>
        {
            entity.HasKey(e => e.DeploymentId).HasName("deployment_pkey");

            entity.ToTable("deployment");

            entity.HasIndex(e => e.ProjectId, "ix_deployment_project");

            entity.HasIndex(e => e.Status, "ix_deployment_status");

            entity.Property(e => e.DeploymentId)
                .ValueGeneratedNever()
                .HasColumnName("deployment_id");
            entity.Property(e => e.AgentId).HasColumnName("agent_id");
            entity.Property(e => e.CloudAccountId).HasColumnName("cloud_account_id");
            entity.Property(e => e.Config)
                .HasColumnType("jsonb")
                .HasColumnName("config");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.EndpointIps)
                .HasColumnType("jsonb")
                .HasColumnName("endpoint_ips");
            entity.Property(e => e.ErrorMessage).HasColumnName("error_message");
            entity.Property(e => e.FinishedAt).HasColumnName("finished_at");
            entity.Property(e => e.Log).HasColumnName("log");
            entity.Property(e => e.Name)
                .HasMaxLength(200)
                .HasColumnName("name");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.ProviderSummary).HasColumnName("provider_summary");
            entity.Property(e => e.StartedAt).HasColumnName("started_at");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'pending'::character varying")
                .HasColumnName("status");
        });

        modelBuilder.Entity<CloudConnection>(entity =>
        {
            entity.HasKey(e => e.ConnectionId).HasName("cloud_connection_pkey");

            entity.ToTable("cloud_connection");

            entity.HasIndex(e => e.Provider, "ix_cloud_connection_provider");

            entity.Property(e => e.ConnectionId)
                .ValueGeneratedNever()
                .HasColumnName("connection_id");
            entity.Property(e => e.Config)
                .HasColumnType("jsonb")
                .HasColumnName("config");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.LastValidated).HasColumnName("last_validated");
            entity.Property(e => e.Name)
                .HasMaxLength(200)
                .HasColumnName("name");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Provider)
                .HasMaxLength(20)
                .HasColumnName("provider");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'pending'::character varying")
                .HasColumnName("status");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
            entity.Property(e => e.ValidationError).HasColumnName("validation_error");
        });

        modelBuilder.Entity<ProjectMember>(entity =>
        {
            entity.HasKey(e => new { e.ProjectId, e.UserId }).HasName("project_member_pkey");

            entity.ToTable("project_member");

            entity.HasIndex(e => e.UserId, "ix_project_member_user");

            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.UserId).HasColumnName("user_id");
            entity.Property(e => e.InviteSentAt).HasColumnName("invite_sent_at");
            entity.Property(e => e.InvitedBy).HasColumnName("invited_by");
            entity.Property(e => e.JoinedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("joined_at");
            entity.Property(e => e.Role)
                .HasMaxLength(20)
                .HasDefaultValueSql("'viewer'::character varying")
                .HasColumnName("role");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'active'::character varying")
                .HasColumnName("status");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("project_member_project_id_fkey");
        });

        modelBuilder.Entity<ShareLink>(entity =>
        {
            entity.HasKey(e => e.LinkId).HasName("share_link_pkey");

            entity.ToTable("share_link");

            entity.HasIndex(e => e.TokenHash, "share_link_token_hash_key").IsUnique();

            entity.HasIndex(e => new { e.ProjectId, e.ResourceType }, "ix_share_link_project");

            entity.Property(e => e.LinkId)
                .ValueGeneratedNever()
                .HasColumnName("link_id");
            entity.Property(e => e.AccessCount)
                .HasDefaultValue(0)
                .HasColumnName("access_count");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.ExpiresAt).HasColumnName("expires_at");
            entity.Property(e => e.Label)
                .HasMaxLength(200)
                .HasColumnName("label");
            entity.Property(e => e.LastAccessed).HasColumnName("last_accessed");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e. ResourceId).HasColumnName("resource_id");
            entity.Property(e => e.ResourceType)
                .HasMaxLength(20)
                .HasColumnName("resource_type");
            entity.Property(e => e.Revoked)
                .HasDefaultValue(false)
                .HasColumnName("revoked");
            entity.Property(e => e.TokenHash)
                .HasMaxLength(64)
                .HasColumnName("token_hash");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("share_link_project_id_fkey");
        });

        modelBuilder.Entity<CommandApproval>(entity =>
        {
            entity.HasKey(e => e.ApprovalId).HasName("command_approval_pkey");

            entity.ToTable("command_approval");

            entity.Property(e => e.ApprovalId)
                .ValueGeneratedNever()
                .HasColumnName("approval_id");
            entity.Property(e => e.AgentId).HasColumnName("agent_id");
            entity.Property(e => e.CommandDetail)
                .HasColumnType("jsonb")
                .HasColumnName("command_detail");
            entity.Property(e => e.CommandType)
                .HasMaxLength(50)
                .HasColumnName("command_type");
            entity.Property(e => e.DecidedAt).HasColumnName("decided_at");
            entity.Property(e => e.DecidedBy).HasColumnName("decided_by");
            entity.Property(e => e.ExpiresAt).HasColumnName("expires_at");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Reason).HasColumnName("reason");
            entity.Property(e => e.RequestedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("requested_at");
            entity.Property(e => e.RequestedBy).HasColumnName("requested_by");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'pending'::character varying")
                .HasColumnName("status");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("command_approval_project_id_fkey");

            entity.HasOne(d => d.Agent).WithMany()
                .HasForeignKey(d => d.AgentId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("command_approval_agent_id_fkey");
        });

        modelBuilder.Entity<TestVisibilityRule>(entity =>
        {
            entity.HasKey(e => e.RuleId).HasName("test_visibility_rule_pkey");

            entity.ToTable("test_visibility_rule");

            entity.HasIndex(e => new { e.ProjectId, e.UserId, e.ResourceType }, "ix_visibility_project");

            entity.Property(e => e.RuleId)
                .ValueGeneratedNever()
                .HasColumnName("rule_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.ResourceId).HasColumnName("resource_id");
            entity.Property(e => e.ResourceType)
                .HasMaxLength(20)
                .HasColumnName("resource_type");
            entity.Property(e => e.UserId).HasColumnName("user_id");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("test_visibility_rule_project_id_fkey");
        });

        modelBuilder.Entity<WorkspaceInvite>(entity =>
        {
            entity.HasKey(e => e.InviteId).HasName("workspace_invite_pkey");

            entity.ToTable("workspace_invite");

            entity.HasIndex(e => e.TokenHash, "ix_workspace_invite_token");

            entity.HasIndex(e => new { e.ProjectId, e.Status }, "ix_workspace_invite_project");

            entity.Property(e => e.InviteId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("invite_id");
            entity.Property(e => e.AcceptedAt).HasColumnName("accepted_at");
            entity.Property(e => e.AcceptedBy).HasColumnName("accepted_by");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.Email)
                .HasMaxLength(255)
                .HasColumnName("email");
            entity.Property(e => e.ExpiresAt).HasColumnName("expires_at");
            entity.Property(e => e.InvitedBy).HasColumnName("invited_by");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Role)
                .HasMaxLength(20)
                .HasDefaultValueSql("'viewer'::character varying")
                .HasColumnName("role");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'pending'::character varying")
                .HasColumnName("status");
            entity.Property(e => e.TokenHash)
                .HasMaxLength(128)
                .HasColumnName("token_hash");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("workspace_invite_project_id_fkey");
        });

        modelBuilder.Entity<WorkspaceWarning>(entity =>
        {
            entity.HasKey(e => e.WarningId).HasName("workspace_warning_pkey");

            entity.ToTable("workspace_warning");

            entity.HasIndex(e => new { e.ProjectId, e.WarningType }, "ix_workspace_warning_unique").IsUnique();

            entity.Property(e => e.WarningId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("warning_id");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.SentAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("sent_at");
            entity.Property(e => e.WarningType)
                .HasMaxLength(30)
                .HasColumnName("warning_type");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("workspace_warning_project_id_fkey");
        });

        modelBuilder.Entity<BenchmarkVmCatalog>(entity =>
        {
            entity.HasKey(e => e.VmId).HasName("benchmark_vm_catalog_pkey");

            entity.ToTable("benchmark_vm_catalog");

            entity.HasIndex(e => e.ProjectId, "ix_benchmark_vm_catalog_project");

            entity.Property(e => e.VmId)
                .ValueGeneratedNever()
                .HasColumnName("vm_id");
            entity.Property(e => e.Cloud)
                .HasMaxLength(20)
                .HasColumnName("cloud");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.Ip)
                .HasMaxLength(200)
                .HasColumnName("ip");
            entity.Property(e => e.Languages)
                .HasDefaultValueSql("'[]'::jsonb")
                .HasColumnType("jsonb")
                .HasColumnName("languages");
            entity.Property(e => e.LastHealthCheck).HasColumnName("last_health_check");
            entity.Property(e => e.Name)
                .HasMaxLength(200)
                .HasColumnName("name");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Region)
                .HasMaxLength(100)
                .HasColumnName("region");
            entity.Property(e => e.SshUser)
                .HasMaxLength(100)
                .HasDefaultValueSql("'azureuser'::character varying")
                .HasColumnName("ssh_user");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'unknown'::character varying")
                .HasColumnName("status");
            entity.Property(e => e.VmSize)
                .HasMaxLength(100)
                .HasColumnName("vm_size");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("benchmark_vm_catalog_project_id_fkey");
        });

        modelBuilder.Entity<SovereigntyZone>(entity =>
        {
            entity.HasKey(e => e.Code).HasName("sovereignty_zone_pkey");

            entity.ToTable("sovereignty_zone");

            entity.HasIndex(e => e.Name, "sovereignty_zone_name_key").IsUnique();

            entity.Property(e => e.Code)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("code");
            entity.Property(e => e.AutoDetect)
                .HasDefaultValueSql("'{}'::jsonb")
                .HasColumnType("jsonb")
                .HasColumnName("auto_detect");
            entity.Property(e => e.ComplianceLevel)
                .HasMaxLength(100)
                .HasColumnName("compliance_level");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.Display)
                .HasMaxLength(100)
                .HasColumnName("display");
            entity.Property(e => e.FallbackZone)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("fallback_zone");
            entity.Property(e => e.LegalNote)
                .HasMaxLength(255)
                .HasColumnName("legal_note");
            entity.Property(e => e.Name)
                .HasMaxLength(50)
                .HasColumnName("name");
            entity.Property(e => e.ParentCode)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("parent_code");
            entity.Property(e => e.RequiresApproval)
                .HasDefaultValue(false)
                .HasColumnName("requires_approval");
            entity.Property(e => e.RequiresMfa)
                .HasDefaultValue(false)
                .HasColumnName("requires_mfa");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'active'::character varying")
                .HasColumnName("status");

            entity.HasOne<SovereigntyZone>().WithMany()
                .HasForeignKey(d => d.ParentCode)
                .HasConstraintName("sovereignty_zone_parent_code_fkey");

            entity.HasOne<SovereigntyZone>().WithMany()
                .HasForeignKey(d => d.FallbackZone)
                .HasConstraintName("sovereignty_zone_fallback_zone_fkey");
        });

        modelBuilder.Entity<ServerRegistry>(entity =>
        {
            entity.HasKey(e => new { e.ZoneCode, e.ServerId }).HasName("server_registry_pkey");

            entity.ToTable("server_registry");

            entity.HasIndex(e => new { e.ZoneCode, e.Status }, "ix_server_registry_status");

            entity.Property(e => e.ZoneCode)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("zone_code");
            entity.Property(e => e.ServerId)
                .HasMaxLength(3)
                .IsFixedLength()
                .HasColumnName("server_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.DbUrl)
                .HasMaxLength(500)
                .HasColumnName("db_url");
            entity.Property(e => e.Endpoint)
                .HasMaxLength(255)
                .HasColumnName("endpoint");
            entity.Property(e => e.Hostname)
                .HasMaxLength(255)
                .HasColumnName("hostname");
            entity.Property(e => e.InternalIp)
                .HasMaxLength(45)
                .HasColumnName("internal_ip");
            entity.Property(e => e.LastHealth).HasColumnName("last_health");
            entity.Property(e => e.Priority)
                .HasDefaultValue(0)
                .HasColumnName("priority");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'active'::character varying")
                .HasColumnName("status");

            entity.HasOne(d => d.Zone).WithMany(p => p.ServerRegistries)
                .HasForeignKey(d => d.ZoneCode)
                .HasConstraintName("server_registry_zone_code_fkey");
        });

        modelBuilder.Entity<ProjectRouting>(entity =>
        {
            entity.HasKey(e => e.ProjectId).HasName("project_routing_pkey");

            entity.ToTable("project_routing");

            entity.HasIndex(e => new { e.CurrentZone, e.HomeZone }, "ix_project_routing_current");

            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.CurrentZone)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("current_zone");
            entity.Property(e => e.HomeZone)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("home_zone");
            entity.Property(e => e.MigratedAt).HasColumnName("migrated_at");
            entity.Property(e => e.MigratedBy).HasColumnName("migrated_by");

            entity.HasOne<SovereigntyZone>().WithMany()
                .HasForeignKey(d => d.HomeZone)
                .HasConstraintName("project_routing_home_zone_fkey");

            entity.HasOne<SovereigntyZone>().WithMany()
                .HasForeignKey(d => d.CurrentZone)
                .HasConstraintName("project_routing_current_zone_fkey");
        });

        modelBuilder.Entity<MigrationRequest>(entity =>
        {
            entity.HasKey(e => e.RequestId).HasName("migration_request_pkey");

            entity.ToTable("migration_request");

            entity.Property(e => e.RequestId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("request_id");
            entity.Property(e => e.ApprovedBy).HasColumnName("approved_by");
            entity.Property(e => e.CompletedAt).HasColumnName("completed_at");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.DataSizeMb).HasColumnName("data_size_mb");
            entity.Property(e => e.ErrorMessage).HasColumnName("error_message");
            entity.Property(e => e.FromZone)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("from_zone");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Reason).HasColumnName("reason");
            entity.Property(e => e.RequestedBy).HasColumnName("requested_by");
            entity.Property(e => e.ScheduledAt).HasColumnName("scheduled_at");
            entity.Property(e => e.StartedAt).HasColumnName("started_at");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasDefaultValueSql("'pending'::character varying")
                .HasColumnName("status");
            entity.Property(e => e.ToZone)
                .HasMaxLength(2)
                .IsFixedLength()
                .HasColumnName("to_zone");

            entity.HasOne<SovereigntyZone>().WithMany()
                .HasForeignKey(d => d.FromZone)
                .HasConstraintName("migration_request_from_zone_fkey");

            entity.HasOne<SovereigntyZone>().WithMany()
                .HasForeignKey(d => d.ToZone)
                .HasConstraintName("migration_request_to_zone_fkey");
        });

        modelBuilder.Entity<MigrationAuditLog>(entity =>
        {
            entity.HasKey(e => e.LogId).HasName("migration_audit_log_pkey");

            entity.ToTable("migration_audit_log");

            entity.HasIndex(e => new { e.RequestId, e.CreatedAt }, "ix_migration_audit_request");

            entity.Property(e => e.LogId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("log_id");
            entity.Property(e => e.Checksum)
                .HasMaxLength(128)
                .HasColumnName("checksum");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.Details)
                .HasColumnType("jsonb")
                .HasColumnName("details");
            entity.Property(e => e.DurationMs).HasColumnName("duration_ms");
            entity.Property(e => e.RequestId).HasColumnName("request_id");
            entity.Property(e => e.Status)
                .HasMaxLength(20)
                .HasColumnName("status");
            entity.Property(e => e.Step)
                .HasMaxLength(50)
                .HasColumnName("step");

            entity.HasOne(d => d.Request).WithMany(p => p.MigrationAuditLogs)
                .HasForeignKey(d => d.RequestId)
                .HasConstraintName("migration_audit_log_request_id_fkey");
        });

        modelBuilder.Entity<SystemHealth>(entity =>
        {
            entity.HasKey(e => e.Id).HasName("system_health_pkey");

            entity.ToTable("system_health");

            entity.HasIndex(e => e.CheckedAt, "ix_system_health_checked_at").IsDescending();

            entity.HasIndex(e => new { e.CheckName, e.CheckedAt }, "ix_system_health_name")
                .IsDescending(false, true);

            entity.Property(e => e.Id).HasColumnName("id");
            entity.Property(e => e.CheckName)
                .HasMaxLength(50)
                .HasColumnName("check_name");
            entity.Property(e => e.CheckedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("checked_at");
            entity.Property(e => e.Details)
                .HasColumnType("jsonb")
                .HasColumnName("details");
            entity.Property(e => e.Message).HasColumnName("message");
            entity.Property(e => e.Status)
                .HasMaxLength(10)
                .HasColumnName("status");
            entity.Property(e => e.Value).HasColumnName("value");
        });

        modelBuilder.Entity<SsoProvider>(entity =>
        {
            entity.HasKey(e => e.ProviderId).HasName("sso_provider_pkey");

            entity.ToTable("sso_provider");

            entity.Property(e => e.ProviderId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("provider_id");
            entity.Property(e => e.ClientId).HasColumnName("client_id");
            entity.Property(e => e.ClientSecretEnc).HasColumnName("client_secret_enc");
            entity.Property(e => e.ClientSecretNonce).HasColumnName("client_secret_nonce");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.DisplayOrder)
                .HasDefaultValue((short)0)
                .HasColumnName("display_order");
            entity.Property(e => e.Enabled)
                .HasDefaultValue(true)
                .HasColumnName("enabled");
            entity.Property(e => e.ExtraConfig)
                .HasDefaultValueSql("'{}'::jsonb")
                .HasColumnType("jsonb")
                .HasColumnName("extra_config");
            entity.Property(e => e.IssuerUrl).HasColumnName("issuer_url");
            entity.Property(e => e.Name)
                .HasMaxLength(200)
                .HasColumnName("name");
            entity.Property(e => e.ProviderType)
                .HasMaxLength(30)
                .HasColumnName("provider_type");
            entity.Property(e => e.TenantId).HasColumnName("tenant_id");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
        });

        modelBuilder.Entity<SystemConfig>(entity =>
        {
            entity.HasKey(e => e.Key).HasName("system_config_pkey");

            entity.ToTable("system_config");

            entity.Property(e => e.Key)
                .HasMaxLength(100)
                .HasColumnName("key");
            entity.Property(e => e.UpdatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("updated_at");
            entity.Property(e => e.UpdatedBy).HasColumnName("updated_by");
            entity.Property(e => e.Value).HasColumnName("value");
        });

        modelBuilder.Entity<AgentCommand>(entity =>
        {
            entity.HasKey(e => e.CommandId).HasName("agent_command_pkey");

            entity.ToTable("agent_command");

            entity.HasIndex(e => new { e.AgentId, e.CreatedAt }, "idx_agent_command_agent")
                .IsDescending(false, true);

            entity.Property(e => e.CommandId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("command_id");
            entity.Property(e => e.AgentId).HasColumnName("agent_id");
            entity.Property(e => e.Args)
                .HasDefaultValueSql("'{}'::jsonb")
                .HasColumnType("jsonb")
                .HasColumnName("args");
            entity.Property(e => e.ConfigId).HasColumnName("config_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.ErrorMessage).HasColumnName("error_message");
            entity.Property(e => e.FinishedAt).HasColumnName("finished_at");
            entity.Property(e => e.Result)
                .HasColumnType("jsonb")
                .HasColumnName("result");
            entity.Property(e => e.StartedAt).HasColumnName("started_at");
            entity.Property(e => e.Status)
                .HasDefaultValueSql("'pending'::text")
                .HasColumnName("status");
            entity.Property(e => e.Verb).HasColumnName("verb");

            entity.HasOne(d => d.Agent).WithMany()
                .HasForeignKey(d => d.AgentId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("agent_command_agent_id_fkey");
        });

        modelBuilder.Entity<VmLifecycle>(entity =>
        {
            entity.HasKey(e => e.EventId).HasName("vm_lifecycle_pkey");

            entity.ToTable("vm_lifecycle");

            entity.HasIndex(e => new { e.ProjectId, e.EventTime }, "idx_vm_lifecycle_project_time")
                .IsDescending(false, true);

            entity.HasIndex(e => new { e.ResourceType, e.ResourceId, e.EventTime }, "idx_vm_lifecycle_resource")
                .IsDescending(false, false, true);

            entity.Property(e => e.EventId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("event_id");
            entity.Property(e => e.Cloud).HasColumnName("cloud");
            entity.Property(e => e.CloudAccountNameAtEvent).HasColumnName("cloud_account_name_at_event");
            entity.Property(e => e.CloudConnectionId).HasColumnName("cloud_connection_id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.EventTime).HasColumnName("event_time");
            entity.Property(e => e.EventType).HasColumnName("event_type");
            entity.Property(e => e.Metadata)
                .HasColumnType("jsonb")
                .HasColumnName("metadata");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(20)
                .HasColumnName("project_id");
            entity.Property(e => e.ProviderAccountId).HasColumnName("provider_account_id");
            entity.Property(e => e.Region).HasColumnName("region");
            entity.Property(e => e.ResourceId).HasColumnName("resource_id");
            entity.Property(e => e.ResourceName).HasColumnName("resource_name");
            entity.Property(e => e.ResourceType).HasColumnName("resource_type");
            entity.Property(e => e.TriggeredBy).HasColumnName("triggered_by");
            entity.Property(e => e.VmName).HasColumnName("vm_name");
            entity.Property(e => e.VmResourceId).HasColumnName("vm_resource_id");
            entity.Property(e => e.VmSize).HasColumnName("vm_size");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("vm_lifecycle_project_id_fkey");
        });

        modelBuilder.Entity<CostRate>(entity =>
        {
            entity.HasKey(e => e.CostRateId).HasName("cost_rate_pkey");

            entity.ToTable("cost_rate");

            entity.HasIndex(e => new { e.Cloud, e.VmSize, e.Region, e.EffectiveFrom }, "idx_cost_rate_lookup")
                .IsDescending(false, false, false, true);

            entity.Property(e => e.CostRateId)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("cost_rate_id");
            entity.Property(e => e.Cloud).HasColumnName("cloud");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.EffectiveFrom).HasColumnName("effective_from");
            entity.Property(e => e.EffectiveTo).HasColumnName("effective_to");
            entity.Property(e => e.RatePerHourUsd)
                .HasPrecision(12, 6)
                .HasColumnName("rate_per_hour_usd");
            entity.Property(e => e.Region).HasColumnName("region");
            entity.Property(e => e.Source).HasColumnName("source");
            entity.Property(e => e.VmSize).HasColumnName("vm_size");
        });

        modelBuilder.Entity<BenchmarkArtifact>(entity =>
        {
            entity.HasKey(e => e.Id).HasName("benchmark_artifact_pkey");

            entity.ToTable("benchmark_artifact");

            entity.HasIndex(e => e.TestRunId, "ix_benchmark_artifact_run");

            entity.Property(e => e.Id)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("id");
            entity.Property(e => e.Cases)
                .HasColumnType("jsonb")
                .HasColumnName("cases");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.DataQuality)
                .HasColumnType("jsonb")
                .HasColumnName("data_quality");
            entity.Property(e => e.Environment)
                .HasColumnType("jsonb")
                .HasColumnName("environment");
            entity.Property(e => e.Launches)
                .HasColumnType("jsonb")
                .HasColumnName("launches");
            entity.Property(e => e.Methodology)
                .HasColumnType("jsonb")
                .HasColumnName("methodology");
            entity.Property(e => e.Samples)
                .HasColumnType("jsonb")
                .HasColumnName("samples");
            entity.Property(e => e.Summaries)
                .HasColumnType("jsonb")
                .HasColumnName("summaries");
            entity.Property(e => e.TestRunId).HasColumnName("test_run_id");

            entity.HasOne(d => d.TestRun).WithMany()
                .HasForeignKey(d => d.TestRunId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("benchmark_artifact_test_run_id_fkey");
        });

        modelBuilder.Entity<TestSchedule>(entity =>
        {
            entity.HasKey(e => e.Id).HasName("test_schedule_pkey");

            entity.ToTable("test_schedule");

            entity.HasIndex(e => e.TestConfigId, "ix_test_schedule_config");

            entity.HasIndex(e => new { e.Enabled, e.NextFireAt }, "ix_test_schedule_enabled_next");

            entity.Property(e => e.Id)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("id");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.CronExpr).HasColumnName("cron_expr");
            entity.Property(e => e.Enabled)
                .HasDefaultValue(true)
                .HasColumnName("enabled");
            entity.Property(e => e.LastFiredAt).HasColumnName("last_fired_at");
            entity.Property(e => e.LastRunId).HasColumnName("last_run_id");
            entity.Property(e => e.NextFireAt).HasColumnName("next_fire_at");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.TestConfigId).HasColumnName("test_config_id");
            entity.Property(e => e.Timezone)
                .HasDefaultValueSql("'UTC'::text")
                .HasColumnName("timezone");

            entity.HasOne(d => d.TestConfig).WithMany()
                .HasForeignKey(d => d.TestConfigId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("test_schedule_test_config_id_fkey");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("test_schedule_project_id_fkey");

            entity.HasOne(d => d.LastRun).WithMany()
                .HasForeignKey(d => d.LastRunId)
                .OnDelete(DeleteBehavior.SetNull)
                .HasConstraintName("test_schedule_last_run_id_fkey");
        });

        modelBuilder.Entity<ComparisonGroup>(entity =>
        {
            entity.HasKey(e => e.Id).HasName("comparison_group_pkey");

            entity.ToTable("comparison_group");

            entity.HasIndex(e => e.ProjectId, "ix_comparison_group_project");

            entity.Property(e => e.Id)
                .HasDefaultValueSql("gen_random_uuid()")
                .HasColumnName("id");
            entity.Property(e => e.BaseWorkload)
                .HasColumnType("jsonb")
                .HasColumnName("base_workload");
            entity.Property(e => e.Cells)
                .HasColumnType("jsonb")
                .HasColumnName("cells");
            entity.Property(e => e.CreatedAt)
                .HasDefaultValueSql("now()")
                .HasColumnName("created_at");
            entity.Property(e => e.CreatedBy).HasColumnName("created_by");
            entity.Property(e => e.Methodology)
                .HasColumnType("jsonb")
                .HasColumnName("methodology");
            entity.Property(e => e.Name).HasColumnName("name");
            entity.Property(e => e.ProjectId)
                .HasMaxLength(14)
                .IsFixedLength()
                .HasColumnName("project_id");
            entity.Property(e => e.Status)
                .HasDefaultValueSql("'pending'::text")
                .HasColumnName("status");

            entity.HasOne(d => d.Project).WithMany()
                .HasForeignKey(d => d.ProjectId)
                .OnDelete(DeleteBehavior.Cascade)
                .HasConstraintName("comparison_group_project_id_fkey");

            entity.HasMany(d => d.TestRuns).WithOne()
                .HasForeignKey(d => d.ComparisonGroupId)
                .OnDelete(DeleteBehavior.SetNull)
                .HasConstraintName("test_run_comparison_group_id_fkey");
        });

        modelBuilder.HasSequence("chunk_constraint_name", "_timescaledb_catalog");

        OnModelCreatingPartial(modelBuilder);
    }

    partial void OnModelCreatingPartial(ModelBuilder modelBuilder);
}
