using System;
using System.Collections.Generic;

namespace Networker.ControlPlane.Infra;

/// <summary>
/// Static per-(cloud, size) network expectations backing the run report's
/// infrastructure envelope ("expected vs measured" throughput).
///
/// <c>EgressMbps</c> is the cloud's *outbound* bandwidth expectation for the
/// size. <c>Confidence</c> is honest about provenance:
/// <list type="bullet">
/// <item><c>documented</c> — the provider's published size table states the
/// number (e.g. Azure Dsv5/Esv5 "expected network bandwidth").</item>
/// <item><c>estimated</c> — the provider does not guarantee bandwidth for the
/// size (Azure B-series burstables, AWS t-series baselines); figures are
/// community consensus, calibrated where we have live measurements (a
/// Standard_B2s target measured 530–580 Mbps egress on 2026-07-31 — see the
/// throughput-direction investigation).</item>
/// </list>
///
/// Ingress is deliberately absent: the major clouds do not meaningfully cap VM
/// ingress, so a transfer's practical infrastructure ceiling is the *sending*
/// side's egress — download ceiling = target egress, upload ceiling = runner
/// egress. The dashboard derives per-direction expectations from that rule.
///
/// Unknown (cloud, size) → null; the envelope then simply shows "no spec" for
/// that side instead of inventing a ceiling.
/// </summary>
public static class VmNetworkSpecs
{
    /// <summary>One size's specs. <c>AcceleratedNetworking</c> = the size
    /// *supports* Azure accelerated networking (SR-IOV) — whether a given NIC
    /// has it enabled is a per-VM fact this static table cannot know.</summary>
    public sealed record VmSpec(
        int Vcpus,
        double MemoryGb,
        int EgressMbps,
        string Confidence,
        bool AcceleratedNetworking);

    // Keys are lowercase (cloud, size). Sizes cover what our provisioners offer
    // plus the common neighbours an infra admin would compare against.
    private static readonly Dictionary<(string Cloud, string Size), VmSpec> Catalog = new()
    {
        // ── azure · B-series burstable — bandwidth NOT guaranteed by Azure ──
        [("azure", "standard_b1s")] = new(1, 1, 250, "estimated", false),
        [("azure", "standard_b1ms")] = new(1, 2, 400, "estimated", false),
        [("azure", "standard_b2s")] = new(2, 4, 600, "estimated", false),
        [("azure", "standard_b2ms")] = new(2, 8, 800, "estimated", false),
        [("azure", "standard_b4ms")] = new(4, 16, 1200, "estimated", false),
        // ── azure · D-series general purpose ──
        [("azure", "standard_d2s_v3")] = new(2, 8, 1000, "estimated", true),
        [("azure", "standard_d2s_v4")] = new(2, 8, 5000, "estimated", true),
        [("azure", "standard_d2s_v5")] = new(2, 8, 12500, "documented", true),
        [("azure", "standard_d4s_v5")] = new(4, 16, 12500, "documented", true),
        [("azure", "standard_d8s_v5")] = new(8, 32, 12500, "documented", true),
        // ── azure · E/F ──
        [("azure", "standard_e2s_v5")] = new(2, 16, 12500, "documented", true),
        [("azure", "standard_f2s_v2")] = new(2, 4, 875, "estimated", true),
        // ── aws — t-series numbers are the sustained BASELINE (bursts higher) ──
        [("aws", "t3.micro")] = new(2, 1, 64, "estimated", false),
        [("aws", "t3.small")] = new(2, 2, 128, "estimated", false),
        [("aws", "t3.medium")] = new(2, 4, 256, "estimated", false),
        [("aws", "m5.large")] = new(2, 8, 750, "estimated", true),
        [("aws", "c5.large")] = new(2, 4, 750, "estimated", true),
        // ── gcp — egress scales with vCPU (≈2 Gbps/vCPU class rule) ──
        [("gcp", "e2-micro")] = new(2, 1, 1000, "estimated", false),
        [("gcp", "e2-small")] = new(2, 2, 1000, "estimated", false),
        [("gcp", "e2-medium")] = new(2, 4, 2000, "estimated", false),
        [("gcp", "n2-standard-2")] = new(2, 8, 4000, "estimated", true),
    };

    /// <summary>Case-insensitive lookup; null when either key part is missing
    /// or the (cloud, size) pair is not in the catalog.</summary>
    public static VmSpec? Lookup(string? cloud, string? vmSize)
    {
        if (string.IsNullOrWhiteSpace(cloud) || string.IsNullOrWhiteSpace(vmSize))
        {
            return null;
        }

        return Catalog.TryGetValue(
            (cloud.Trim().ToLowerInvariant(), vmSize.Trim().ToLowerInvariant()),
            out var spec)
            ? spec
            : null;
    }

    /// <summary>Wire shape for one side of the envelope (snake_case, null
    /// specs omitted by the serializer's null-ignore policy).</summary>
    public static object? ToWire(VmSpec? spec) => spec is null
        ? null
        : new
        {
            vcpus = spec.Vcpus,
            memory_gb = spec.MemoryGb,
            egress_mbps = spec.EgressMbps,
            confidence = spec.Confidence,
            accelerated_networking = spec.AcceleratedNetworking,
        };
}
