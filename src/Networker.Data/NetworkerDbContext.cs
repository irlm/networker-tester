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
        modelBuilder.HasSequence("chunk_constraint_name", "_timescaledb_catalog");

        OnModelCreatingPartial(modelBuilder);
    }

    partial void OnModelCreatingPartial(ModelBuilder modelBuilder);
}
