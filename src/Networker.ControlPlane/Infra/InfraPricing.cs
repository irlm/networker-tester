using System;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.EntityFrameworkCore;
using Networker.ControlPlane.Reports;
using Networker.Data;

namespace Networker.ControlPlane.Infra;

/// <summary>
/// Hourly-USD resolution for the infrastructure advisor. Chain per size:
/// DB <c>cost_rates</c> (region-specific wins, newest effective_from breaks
/// ties) → curated <see cref="CloudCostTable"/> (embedded cloud-costs.json,
/// nearest-region fallback) → null. Deliberately NOT
/// <c>CostEstimation.HardcodedHourlyUsd</c>: that fallback answers a D2s_v3
/// price for ANY unknown size, which is fine for a rough tester estimate but
/// would fabricate economics in upsize/downsize advice — the advisor must
/// show "price unknown" instead.
/// </summary>
internal static class InfraPricing
{
    /// <summary>One DB round-trip per (cloud, region); the returned resolver
    /// answers any size from the loaded rows + the embedded table.</summary>
    internal static async Task<Func<string, double?>> ResolverAsync(
        NetworkerDbContext db, string cloud, string? region, CancellationToken ct)
    {
        var now = DateTime.UtcNow;
        var rows = await db.CostRates
            .AsNoTracking()
            .Where(r => r.Cloud == cloud
                        && r.EffectiveFrom <= now
                        && (r.EffectiveTo == null || r.EffectiveTo > now)
                        && (r.Region == null || r.Region == region))
            .ToListAsync(ct);

        return size =>
        {
            var fromDb = rows
                .Where(r => string.Equals(r.VmSize, size, StringComparison.OrdinalIgnoreCase))
                .OrderByDescending(r => r.Region != null)
                .ThenByDescending(r => r.EffectiveFrom)
                .Select(r => (double?)(double)r.RatePerHourUsd)
                .FirstOrDefault();
            if (fromDb.HasValue)
            {
                return fromDb;
            }

            var curated = CloudCostTable.Instance.Find(cloud, size, region ?? string.Empty);
            return curated is null ? null : (double)curated.Rate.HourlyUsd;
        };
    }
}
