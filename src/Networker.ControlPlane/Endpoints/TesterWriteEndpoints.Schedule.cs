using System.Diagnostics;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Auth;
using Networker.ControlPlane.Provisioning;
using Networker.Data;
using Networker.Data.Entities;
using Networker.Security;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Endpoints;

// Auto-shutdown scheduling handlers (postpone / schedule PATCH) + the
// region -> timezone -> next-shutdown computation for TesterWriteEndpoints
// (route mapping + shared helpers live in TesterWriteEndpoints.cs).
public static partial class TesterWriteEndpoints
{
    // ── postpone ────────────────────────────────────────────────────────────────

    /// <summary>POST /postpone — extend auto-shutdown. Body is one of
    /// <c>{until}</c>, <c>{add_hours}</c>, or <c>{skip_tonight:true}</c>. Bumps
    /// <c>shutdown_deferral_count</c> and returns the updated row (200).</summary>
    private static async Task<IResult> PostponeShutdown(
        string projectId,
        Guid testerId,
        HttpContext http,
        [FromBody] PostponeBody? body,
        NetworkerDbContext db,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Postpone");

        if (body is null)
        {
            return Results.BadRequest(new { error = "postpone body required" });
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        var now = DateTime.UtcNow;
        DateTime newNext;
        try
        {
            newNext = ComputePostpone(body, tester, now);
        }
        catch (ArgumentException ex)
        {
            return Results.BadRequest(new { error = ex.Message });
        }

        tester.NextShutdownAt = newNext;
        tester.ShutdownDeferralCount = (short)(tester.ShutdownDeferralCount + 1);
        tester.UpdatedAt = now;
        await db.SaveChangesAsync(ct);

        logger.LogInformation(
            "tester {TesterId} shutdown postponed to {Next} by {Actor}", testerId, newNext, user?.Email);

        return Results.Ok(ToDto(tester));
    }

    /// <summary>Pure postpone computation — mirrors the Rust <c>compute_postpone</c>.
    /// Exactly one of the three body shapes must be populated.</summary>
    internal static DateTime ComputePostpone(PostponeBody body, ProjectTester tester, DateTime now)
    {
        if (body.Until is { } until)
        {
            var untilUtc = until.ToUniversalTime();
            if (untilUtc <= now)
            {
                throw new ArgumentException("until must be in the future");
            }
            return untilUtc;
        }
        if (body.AddHours is { } hours)
        {
            if (hours <= 0)
            {
                throw new ArgumentException("add_hours must be positive");
            }
            var baseline = tester.NextShutdownAt ?? now;
            return baseline.AddHours(hours);
        }
        if (body.SkipTonight is true)
        {
            // Roll one day forward and recompute tomorrow's slot.
            return NextShutdownAtForProvider(tester.Cloud, tester.Region, tester.AutoShutdownLocalHour, now.AddHours(24));
        }
        throw new ArgumentException("exactly one of until / add_hours / skip_tonight required");
    }

    // ── schedule (PATCH) ──────────────────────────────────────────────────────

    /// <summary>PATCH /schedule — set auto-shutdown enabled + local hour;
    /// recomputes <c>next_shutdown_at</c> in the region's timezone (cleared when
    /// disabled). Returns the updated row (200).</summary>
    private static async Task<IResult> UpdateSchedule(
        string projectId,
        Guid testerId,
        HttpContext http,
        [FromBody] ScheduleBody? body,
        NetworkerDbContext db,
        ILoggerFactory loggerFactory,
        CancellationToken ct)
    {
        var user = http.GetAuthUser();
        var logger = loggerFactory.CreateLogger("TesterWrite.Schedule");

        if (body is null || (body.AutoShutdownEnabled is null && body.AutoShutdownLocalHour is null))
        {
            return Results.BadRequest(new
            {
                error = "at least one of auto_shutdown_enabled or auto_shutdown_local_hour required",
            });
        }
        if (body.AutoShutdownLocalHour is { } h && (h < 0 || h > 23))
        {
            return Results.BadRequest(new { error = "auto_shutdown_local_hour must be 0..=23" });
        }

        var tester = await LoadAsync(db, projectId, testerId, ct);
        if (tester is null)
        {
            return Results.NotFound(new { error = "Tester not found" });
        }

        var newEnabled = body.AutoShutdownEnabled ?? tester.AutoShutdownEnabled;
        var newHour = body.AutoShutdownLocalHour ?? tester.AutoShutdownLocalHour;

        tester.AutoShutdownEnabled = newEnabled;
        tester.AutoShutdownLocalHour = newHour;
        tester.NextShutdownAt = newEnabled
            ? NextShutdownAtForProvider(tester.Cloud, tester.Region, newHour, DateTime.UtcNow)
            : null;
        tester.UpdatedAt = DateTime.UtcNow;
        await db.SaveChangesAsync(ct);

        logger.LogInformation(
            "tester {TesterId} schedule updated by {Actor}: enabled={Enabled} hour={Hour}",
            testerId, user?.Email, newEnabled, newHour);

        return Results.Ok(ToDto(tester));
    }

    // ── region → timezone → next shutdown (ported from azure_regions.rs) ──────

    /// <summary>
    /// Next UTC instant at <paramref name="localHour"/>:00 in the region's local
    /// timezone, rolling forward one day if today's slot has passed. Port of the
    /// Rust <c>next_shutdown_at_for_provider</c>.
    /// </summary>
    internal static DateTime NextShutdownAtForProvider(string? cloud, string region, short localHour, DateTime nowUtc)
    {
        var tz = RegionTimeZone(cloud, region);
        var hour = Math.Clamp((int)localHour, 0, 23);
        var localNow = TimeZoneInfo.ConvertTimeFromUtc(DateTime.SpecifyKind(nowUtc, DateTimeKind.Utc), tz);

        var todayLocal = new DateTime(localNow.Year, localNow.Month, localNow.Day, hour, 0, 0, DateTimeKind.Unspecified);
        if (todayLocal > localNow)
        {
            return TimeZoneInfo.ConvertTimeToUtc(todayLocal, tz);
        }

        var tomorrowLocal = todayLocal.AddDays(1);
        return TimeZoneInfo.ConvertTimeToUtc(tomorrowLocal, tz);
    }

    /// <summary>
    /// Cloud region → <see cref="TimeZoneInfo"/> via IANA ids (cross-platform on
    /// .NET). Mirrors the Rust <c>region_timezone_for_provider</c> mappings;
    /// unknown provider/region → UTC.
    /// </summary>
    private static TimeZoneInfo RegionTimeZone(string? cloud, string region)
    {
        var iana = (cloud?.ToLowerInvariant()) switch
        {
            "aws" => AwsRegionIana(region),
            "gcp" => GcpRegionIana(region),
            _ => AzureRegionIana(region),
        };
        try
        {
            return TimeZoneInfo.FindSystemTimeZoneById(iana);
        }
        catch (TimeZoneNotFoundException)
        {
            return TimeZoneInfo.Utc;
        }
        catch (InvalidTimeZoneException)
        {
            return TimeZoneInfo.Utc;
        }
    }

    private static string AzureRegionIana(string region) => region switch
    {
        "eastus" or "eastus2" or "eastus3" => "America/New_York",
        "centralus" or "southcentralus" or "northcentralus" => "America/Chicago",
        "westus" or "westus2" or "westus3" => "America/Los_Angeles",
        "westcentralus" => "America/Denver",
        "northeurope" => "Europe/Dublin",
        "westeurope" => "Europe/Amsterdam",
        "uksouth" or "ukwest" => "Europe/London",
        "francecentral" or "francesouth" => "Europe/Paris",
        "germanywestcentral" or "germanynorth" => "Europe/Berlin",
        "switzerlandnorth" or "switzerlandwest" => "Europe/Zurich",
        "norwayeast" or "norwaywest" => "Europe/Oslo",
        "swedencentral" => "Europe/Stockholm",
        "polandcentral" => "Europe/Warsaw",
        "italynorth" => "Europe/Rome",
        "spaincentral" => "Europe/Madrid",
        "japaneast" or "japanwest" => "Asia/Tokyo",
        "koreacentral" or "koreasouth" => "Asia/Seoul",
        "eastasia" => "Asia/Hong_Kong",
        "southeastasia" => "Asia/Singapore",
        "centralindia" or "southindia" or "westindia" => "Asia/Kolkata",
        "australiaeast" or "australiasoutheast" or "australiacentral" or "australiacentral2" => "Australia/Sydney",
        "brazilsouth" or "brazilsoutheast" => "America/Sao_Paulo",
        "canadacentral" or "canadaeast" => "America/Toronto",
        "mexicocentral" => "America/Mexico_City",
        "uaenorth" or "uaecentral" => "Asia/Dubai",
        "qatarcentral" => "Asia/Qatar",
        "israelcentral" => "Asia/Jerusalem",
        "southafricanorth" or "southafricawest" => "Africa/Johannesburg",
        _ => "UTC",
    };

    private static string AwsRegionIana(string region) => region switch
    {
        "us-east-1" or "us-east-2" => "America/New_York",
        "us-west-1" or "us-west-2" => "America/Los_Angeles",
        "eu-west-1" => "Europe/Dublin",
        "eu-west-2" => "Europe/London",
        "eu-central-1" => "Europe/Berlin",
        "ap-northeast-1" => "Asia/Tokyo",
        "ap-southeast-1" => "Asia/Singapore",
        "ap-southeast-2" => "Australia/Sydney",
        "sa-east-1" => "America/Sao_Paulo",
        _ => "UTC",
    };

    private static string GcpRegionIana(string region) => region switch
    {
        "us-central1" or "us-east1" or "us-east4" => "America/New_York",
        "us-west1" or "us-west2" or "us-west4" => "America/Los_Angeles",
        "europe-west1" or "europe-west4" => "Europe/Amsterdam",
        "europe-west2" => "Europe/London",
        "europe-west3" => "Europe/Berlin",
        "asia-east1" or "asia-east2" => "Asia/Taipei",
        "asia-northeast1" => "Asia/Tokyo",
        "asia-southeast1" => "Asia/Singapore",
        "australia-southeast1" => "Australia/Sydney",
        _ => "UTC",
    };
}
