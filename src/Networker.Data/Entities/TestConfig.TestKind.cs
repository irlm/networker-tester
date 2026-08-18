namespace Networker.Data.Entities;

public partial class TestConfig
{
    /// <summary>
    /// Product workflow that owns this config: network, url_probe, sdk_probe,
    /// or benchmark. This is independent from <see cref="EndpointKind"/>,
    /// which describes the target transport.
    /// </summary>
    public string TestKind { get; set; } = TestConfigKinds.Network;
}
