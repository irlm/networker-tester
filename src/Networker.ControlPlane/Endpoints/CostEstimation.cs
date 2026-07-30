using Microsoft.EntityFrameworkCore;
using Networker.Data;

namespace Networker.ControlPlane.Endpoints;

/// <summary>
/// Shared VM cost-estimation helpers, used by both the tester cost endpoint
/// (<see cref="TestersEndpoints"/>) and the deployment cost endpoint
/// (<see cref="DeploymentsEndpoints"/>) so the two views can never disagree on
/// a price. DB <c>cost_rates</c> rows win; the hardcoded table (mirroring the
/// Rust <c>hourly_usd</c>) is the fallback.
/// </summary>
internal static class CostEstimation
{
    /// <summary>Effective hourly USD for a (cloud, vm_size, region) triple.</summary>
    internal static async Task<double> HourlyUsdAsync(
        NetworkerDbContext db, string cloud, string vmSize, string? region)
    {
        var now = DateTime.UtcNow;
        var rate = await db.CostRates
            .AsNoTracking()
            .Where(r => r.Cloud == cloud
                        && r.VmSize == vmSize
                        && r.EffectiveFrom <= now
                        && (r.EffectiveTo == null || r.EffectiveTo > now)
                        && (r.Region == null || r.Region == region))
            // Region-specific match wins over a region-agnostic one; newest
            // effective_from breaks further ties.
            .OrderByDescending(r => r.Region != null)
            .ThenByDescending(r => r.EffectiveFrom)
            .Select(r => (decimal?)r.RatePerHourUsd)
            .FirstOrDefaultAsync();

        return rate.HasValue ? (double)rate.Value : HardcodedHourlyUsd(vmSize);
    }

    /// <summary>
    /// Hardcoded hourly USD lookup — mirrors the Rust <c>hourly_usd</c>.
    /// Unknown sizes fall back to the Standard_D2s_v3 rate.
    /// </summary>
    internal static double HardcodedHourlyUsd(string vmSize) => vmSize switch
    {
        "Standard_D2s_v3" => 0.096,
        "Standard_D4s_v3" => 0.192,
        "Standard_D8s_v3" => 0.384,
        _ => 0.096,
    };

    /// <summary>
    /// (always_on, with_schedule) monthly USD. Mirrors the Rust
    /// <c>cost_estimate</c>: 24h×30d always-on, 15h×30d when auto-shutdown is
    /// enabled (business-day approximation), else equal to always-on.
    /// </summary>
    internal static (double AlwaysOn, double WithSchedule) MonthlyEstimate(double hourly, bool autoShutdownEnabled)
    {
        var alwaysOn = 24.0 * 30.0 * hourly;
        var withSchedule = autoShutdownEnabled ? 15.0 * 30.0 * hourly : alwaysOn;
        return (alwaysOn, withSchedule);
    }
}
