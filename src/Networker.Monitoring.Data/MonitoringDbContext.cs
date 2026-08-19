using Microsoft.EntityFrameworkCore;
using Networker.Monitoring.Data.Entities;
using MonitorEntity = Networker.Monitoring.Data.Entities.Monitor;

namespace Networker.Monitoring.Data;

public sealed class MonitoringDbContext(DbContextOptions<MonitoringDbContext> options) : DbContext(options)
{
    public DbSet<MonitorEntity> Monitors => Set<MonitorEntity>();
    public DbSet<MonitorLocation> MonitorLocations => Set<MonitorLocation>();
    public DbSet<MonitorLocationAssignment> MonitorLocationAssignments => Set<MonitorLocationAssignment>();
    public DbSet<MonitorCheck> MonitorChecks => Set<MonitorCheck>();

    protected override void OnModelCreating(ModelBuilder modelBuilder)
    {
        modelBuilder.Entity<MonitorEntity>(entity =>
        {
            entity.ToTable("monitor");
            entity.HasKey(x => x.MonitorId).HasName("monitor_pkey");
            entity.Property(x => x.MonitorId).HasColumnName("monitor_id");
            entity.Property(x => x.ProjectId).HasColumnName("project_id").HasMaxLength(128);
            entity.Property(x => x.Name).HasColumnName("name").HasMaxLength(200);
            entity.Property(x => x.TargetUrl).HasColumnName("target_url").HasMaxLength(2048);
            entity.Property(x => x.Method).HasColumnName("method").HasMaxLength(8);
            entity.Property(x => x.AssertionConfig).HasColumnName("assertion_config").HasColumnType("jsonb");
            entity.Property(x => x.IntervalSeconds).HasColumnName("interval_seconds");
            entity.Property(x => x.TimeoutMs).HasColumnName("timeout_ms");
            entity.Property(x => x.Enabled).HasColumnName("enabled");
            entity.Property(x => x.Criticality).HasColumnName("criticality").HasMaxLength(16);
            entity.Property(x => x.RetentionPolicy).HasColumnName("retention_policy").HasColumnType("jsonb");
            entity.Property(x => x.NextCheckAt).HasColumnName("next_check_at");
            entity.Property(x => x.LeaseOwner).HasColumnName("lease_owner").HasMaxLength(200);
            entity.Property(x => x.LeaseExpiresAt).HasColumnName("lease_expires_at");
            entity.Property(x => x.CreatedAt).HasColumnName("created_at");
            entity.Property(x => x.UpdatedAt).HasColumnName("updated_at");
            entity.Property(x => x.DeletedAt).HasColumnName("deleted_at");
            entity.HasIndex(x => new { x.ProjectId, x.Name })
                .IsUnique()
                .HasFilter("deleted_at IS NULL")
                .HasDatabaseName("monitor_project_name_uq");
            entity.HasIndex(x => new { x.Enabled, x.NextCheckAt })
                .HasFilter("deleted_at IS NULL")
                .HasDatabaseName("monitor_due_idx");
        });

        modelBuilder.Entity<MonitorLocation>(entity =>
        {
            entity.ToTable("monitor_location");
            entity.HasKey(x => x.LocationId).HasName("monitor_location_pkey");
            entity.Property(x => x.LocationId).HasColumnName("location_id");
            entity.Property(x => x.Name).HasColumnName("name").HasMaxLength(200);
            entity.Property(x => x.Provider).HasColumnName("provider").HasMaxLength(100);
            entity.Property(x => x.Region).HasColumnName("region").HasMaxLength(100);
            entity.Property(x => x.LastHeartbeatAt).HasColumnName("last_heartbeat_at");
            entity.Property(x => x.Enabled).HasColumnName("enabled");
            entity.Property(x => x.CreatedAt).HasColumnName("created_at");
            entity.Property(x => x.UpdatedAt).HasColumnName("updated_at");
            entity.HasIndex(x => x.Name).IsUnique().HasDatabaseName("monitor_location_name_uq");
        });

        modelBuilder.Entity<MonitorLocationAssignment>(entity =>
        {
            entity.ToTable("monitor_location_assignment");
            entity.HasKey(x => new { x.MonitorId, x.LocationId }).HasName("monitor_location_assignment_pkey");
            entity.Property(x => x.MonitorId).HasColumnName("monitor_id");
            entity.Property(x => x.LocationId).HasColumnName("location_id");
            entity.Property(x => x.ParticipatesInIncidentQuorum).HasColumnName("participates_in_incident_quorum");
            entity.Property(x => x.CreatedAt).HasColumnName("created_at");
            entity.HasIndex(x => x.MonitorId).IsUnique().HasDatabaseName("monitor_single_location_uq");
            entity.HasOne(x => x.Monitor).WithMany(x => x.LocationAssignments)
                .HasForeignKey(x => x.MonitorId).OnDelete(DeleteBehavior.Cascade);
            entity.HasOne(x => x.Location).WithMany(x => x.MonitorAssignments)
                .HasForeignKey(x => x.LocationId).OnDelete(DeleteBehavior.Restrict);
        });

        modelBuilder.Entity<MonitorCheck>(entity =>
        {
            entity.ToTable("monitor_check");
            entity.HasKey(x => x.CheckId).HasName("monitor_check_pkey");
            entity.Property(x => x.CheckId).HasColumnName("check_id");
            entity.Property(x => x.MonitorId).HasColumnName("monitor_id");
            entity.Property(x => x.LocationId).HasColumnName("location_id");
            entity.Property(x => x.ScheduledAt).HasColumnName("scheduled_at");
            entity.Property(x => x.StartedAt).HasColumnName("started_at");
            entity.Property(x => x.FinishedAt).HasColumnName("finished_at");
            entity.Property(x => x.Outcome).HasColumnName("outcome").HasMaxLength(16);
            entity.Property(x => x.FailureKind).HasColumnName("failure_kind").HasMaxLength(32);
            entity.Property(x => x.StatusCode).HasColumnName("status_code");
            entity.Property(x => x.DnsMs).HasColumnName("dns_ms");
            entity.Property(x => x.ConnectMs).HasColumnName("connect_ms");
            entity.Property(x => x.TlsMs).HasColumnName("tls_ms");
            entity.Property(x => x.TtfbMs).HasColumnName("ttfb_ms");
            entity.Property(x => x.TotalMs).HasColumnName("total_ms");
            entity.Property(x => x.AssertionResults).HasColumnName("assertion_results").HasColumnType("jsonb");
            entity.Property(x => x.ResponseDigest).HasColumnName("response_digest").HasMaxLength(128);
            entity.Property(x => x.ErrorSummary).HasColumnName("error_summary").HasMaxLength(1000);
            entity.Property(x => x.CreatedAt).HasColumnName("created_at");
            entity.HasIndex(x => new { x.MonitorId, x.ScheduledAt })
                .IsDescending(false, true)
                .HasDatabaseName("monitor_check_history_idx");
            entity.HasOne(x => x.Monitor).WithMany(x => x.Checks)
                .HasForeignKey(x => x.MonitorId).OnDelete(DeleteBehavior.Cascade);
            entity.HasOne(x => x.Location).WithMany(x => x.Checks)
                .HasForeignKey(x => x.LocationId).OnDelete(DeleteBehavior.Restrict);
        });
    }
}
