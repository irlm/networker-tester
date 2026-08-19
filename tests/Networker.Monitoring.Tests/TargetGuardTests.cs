using System.Net;
using Networker.Monitoring.Probe;

namespace Networker.Monitoring.Tests;

public sealed class TargetGuardTests
{
    [Theory]
    [InlineData("127.0.0.1")] // loopback
    [InlineData("127.8.8.8")] // whole 127/8
    [InlineData("0.0.0.0")] // unspecified
    [InlineData("10.0.0.8")] // RFC1918
    [InlineData("172.16.0.1")] // RFC1918
    [InlineData("172.31.255.254")] // RFC1918 upper edge
    [InlineData("192.168.1.1")] // RFC1918
    [InlineData("169.254.169.254")] // link-local / cloud metadata
    [InlineData("169.254.0.1")] // link-local
    [InlineData("100.64.0.1")] // CGNAT
    [InlineData("224.0.0.1")] // multicast
    [InlineData("255.255.255.255")] // broadcast
    [InlineData("::1")] // IPv6 loopback
    [InlineData("::")] // IPv6 unspecified
    [InlineData("fe80::1")] // IPv6 link-local
    [InlineData("fd00::1")] // IPv6 ULA
    [InlineData("::ffff:192.168.1.1")] // IPv4-mapped private
    public void Non_global_addresses_are_blocked(string address) =>
        Assert.True(MonitorTargetGuard.IsBlockedAddress(IPAddress.Parse(address)));

    [Theory]
    [InlineData("8.8.8.8")]
    [InlineData("1.1.1.1")]
    [InlineData("93.184.216.34")]
    [InlineData("172.32.0.1")] // just outside 172.16/12
    [InlineData("2606:4700:4700::1111")]
    public void Global_addresses_are_allowed(string address) =>
        Assert.False(MonitorTargetGuard.IsBlockedAddress(IPAddress.Parse(address)));

    [Theory]
    [InlineData("localhost")]
    [InlineData("LOCALHOST")]
    [InlineData("api.localhost")]
    [InlineData("printer.local")]
    [InlineData("127.0.0.1")]
    [InlineData("10.20.30.40")]
    [InlineData("[::1]")]
    [InlineData("169.254.169.254")]
    public void Obviously_non_global_hosts_are_rejected(string host) =>
        Assert.True(MonitorTargetGuard.IsObviouslyNonGlobalHost(host));

    [Theory]
    [InlineData("example.com")]
    [InlineData("api.laghound.com")]
    [InlineData("8.8.8.8")]
    public void Plausible_global_hosts_pass_syntactic_validation(string host) =>
        Assert.False(MonitorTargetGuard.IsObviouslyNonGlobalHost(host));
}
