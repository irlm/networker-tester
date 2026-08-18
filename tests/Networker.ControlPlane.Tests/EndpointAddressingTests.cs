using Networker.ControlPlane.Provisioning;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The endpoint-hostname seam (V050 <c>deployment.endpoint_hosts</c>): the proxy
/// resolver and the pending→network rewrite must hand the tester the recorded
/// DNS name when there is one (TLS SNI is what makes http.sys/IIS serve HTTP/3)
/// and the IP otherwise — pre-V050 rows, GCP, lan — exactly as before. Also the
/// <c>DeployOutput</c> parser that fills the two parallel arrays from install.sh
/// output (and the docker provider's container names).
/// </summary>
public class EndpointAddressingTests
{
    [Fact]
    public void Prefers_the_recorded_hostname_of_the_first_endpoint_over_its_ip()
    {
        Assert.Equal(
            "nwk-ep.eastus.cloudapp.azure.com",
            EndpointAddressing.PreferredHost("""["20.1.2.3"]""", """["nwk-ep.eastus.cloudapp.azure.com"]"""));
        // The lab's Windows target: registered by ip with a labnet alias as host.
        Assert.Equal("target-3.lab", EndpointAddressing.PreferredHost("""["172.31.100.103"]""", """["target-3.lab"]"""));
    }

    [Theory]
    [InlineData("""["10.0.0.5"]""", null)]           // pre-V050 row: no hosts column value
    [InlineData("""["10.0.0.5"]""", "")]             // empty text
    [InlineData("""["10.0.0.5"]""", "[]")]           // recorded nothing
    [InlineData("""["10.0.0.5"]""", "[null]")]       // provider gave no name (GCP)
    [InlineData("""["10.0.0.5"]""", """[""]""")]     // blank entry
    [InlineData("""["10.0.0.5"]""", "not json")]     // malformed → ignored
    public void Falls_back_to_the_ip_when_no_hostname_was_recorded(string ips, string? hosts)
        => Assert.Equal("10.0.0.5", EndpointAddressing.PreferredHost(ips, hosts));

    [Fact]
    public void Arrays_are_parallel_so_the_hostname_belongs_to_the_same_endpoint()
    {
        // Two endpoints: only the second has a DNS name → the FIRST endpoint is
        // still addressed by its ip (a hostname must never be borrowed across
        // endpoints — that would point the tester at the wrong VM).
        Assert.Equal("10.0.0.1", EndpointAddressing.PreferredHost("""["10.0.0.1","10.0.0.2"]""", """[null,"b.example.com"]"""));
        // First ip slot blank/null → skip to the first addressed endpoint and use ITS hostname.
        Assert.Equal("b.example.com", EndpointAddressing.PreferredHost("""["","10.0.0.2"]""", """[null,"b.example.com"]"""));
    }

    [Fact]
    public void Nothing_captured_is_null_and_a_hostname_alone_still_resolves()
    {
        Assert.Null(EndpointAddressing.PreferredHost(null, null));
        Assert.Null(EndpointAddressing.PreferredHost("[]", "[]"));
        Assert.Null(EndpointAddressing.PreferredHost("{\"x\":1}", null));
        Assert.Equal("only.example.com", EndpointAddressing.PreferredHost(null, """["only.example.com"]"""));
    }

    [Fact]
    public void All_hosts_unions_both_forms_for_reference_matching()
    {
        // The teardown deferral matches an active run's endpoint_ref against
        // every name the deployment is known by — dispatch may have used either.
        var all = EndpointAddressing.AllHosts("""["20.1.2.3","20.1.2.4"]""", """["a.eastus.cloudapp.azure.com",null]""");
        Assert.Equal(["20.1.2.3", "20.1.2.4", "a.eastus.cloudapp.azure.com"], all);
        // Azure rows where endpoint_ips already IS the FQDN (FqdnRe replacement) dedupe.
        Assert.Equal(["a.example.com"], EndpointAddressing.AllHosts("""["a.example.com"]""", """["A.example.com"]"""));
    }

    [Theory]
    [InlineData("172.31.100.103", true)]
    [InlineData("::1", true)]
    [InlineData("target-3.lab", false)]
    [InlineData("nwk-ep.eastus.cloudapp.azure.com", false)]
    [InlineData("", false)]
    [InlineData(null, false)]
    public void Ip_literal_detection(string? host, bool expected)
        => Assert.Equal(expected, EndpointAddressing.IsIpLiteral(host));

    // ── DeployOutput: the install.sh output → endpoint_ips / endpoint_hosts parser ──

    [Fact]
    public void Deploy_output_records_the_endpoint_host_line_next_to_its_ip()
    {
        var o = new DeployRunner.DeployOutput();
        o.ProcessLine("  ✓ VM created (Windows Server 2022) — Public IP: 20.1.2.3", "stdout");
        o.ProcessLine("  → endpoint_host: nwk-ep-win.eastus.cloudapp.azure.com (20.1.2.3)", "stdout");
        Assert.Equal(["20.1.2.3"], o.EndpointIps);
        Assert.Equal(["nwk-ep-win.eastus.cloudapp.azure.com"], o.EndpointHosts);
    }

    [Fact]
    public void Deploy_output_keeps_the_legacy_fqdn_replacement_and_mirrors_it_as_host()
    {
        // Historical behaviour: "fqdn (ip)" replaces the bare ip in endpoint_ips
        // (Azure/AWS rows have carried the FQDN there since v0.13.15). The
        // hostname column mirrors it so PreferredHost is the FQDN either way,
        // and a later explicit endpoint_host line for the same endpoint does
        // not create a second entry.
        var o = new DeployRunner.DeployOutput();
        o.ProcessLine("Instance running — Public IP: 3.4.5.6", "stdout");
        o.ProcessLine("Instance running — ec2-3-4-5-6.compute-1.amazonaws.com (3.4.5.6)", "stdout");
        o.ProcessLine("endpoint_host: ec2-3-4-5-6.compute-1.amazonaws.com (3.4.5.6)", "stdout");
        Assert.Equal(["ec2-3-4-5-6.compute-1.amazonaws.com"], o.EndpointIps);
        Assert.Equal(["ec2-3-4-5-6.compute-1.amazonaws.com"], o.EndpointHosts);
    }

    [Fact]
    public void Deploy_output_arrays_stay_parallel_across_endpoints_without_names()
    {
        var o = new DeployRunner.DeployOutput();
        o.ProcessLine("endpoint_ip: 10.0.0.1 (ep-a, nginx on :8444)", "stdout");           // gcp-style: no name
        o.ProcessLine("endpoint_ip: 10.0.0.2 (ep-b, iis on :8445)", "stdout");
        o.ProcessLine("endpoint_host: nwk-b.westeurope.cloudapp.azure.com (10.0.0.2)", "stdout");
        o.ProcessLine("endpoint_host: nwk-b.westeurope.cloudapp.azure.com (10.0.0.2)", "stdout"); // dup line: no-op
        Assert.Equal(["10.0.0.1", "10.0.0.2"], o.EndpointIps);
        Assert.Equal([null, "nwk-b.westeurope.cloudapp.azure.com"], o.EndpointHosts);
        Assert.Equal("10.0.0.1", EndpointAddressing.PreferredHost(
            System.Text.Json.JsonSerializer.Serialize(o.EndpointIps),
            System.Text.Json.JsonSerializer.Serialize(o.EndpointHosts)));
    }

    [Fact]
    public void Deploy_output_endpoint_host_line_alone_creates_the_entry()
    {
        var o = new DeployRunner.DeployOutput();
        o.ProcessLine("endpoint_host: nwk-lab-ep-nginx-a1b2c (172.31.100.120) — container name, docker-network DNS", "stdout");
        Assert.Equal(["172.31.100.120"], o.EndpointIps);
        Assert.Equal(["nwk-lab-ep-nginx-a1b2c"], o.EndpointHosts);
        // Fallback scan must not add a duplicate afterwards.
        o.RunFallbackIpScan();
        Assert.Single(o.EndpointIps);
    }
}
