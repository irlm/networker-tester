using System.Net;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Security.Cryptography.X509Certificates;

namespace Networker.Endpoint;

/// <summary>
/// Generates a self-signed TLS certificate at startup, mirroring the Rust
/// <c>generate_self_signed_cert()</c> (rcgen). SANs: localhost, 127.0.0.1, ::1,
/// and the primary non-loopback LAN IP when detectable, so remote clients using
/// <c>--insecure</c> / SPKI-pin can connect.
/// </summary>
internal static class SelfSignedCert
{
    public static X509Certificate2 Generate()
    {
        using var rsa = RSA.Create(2048);
        var req = new CertificateRequest("CN=networker-endpoint", rsa, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1);

        req.CertificateExtensions.Add(new X509BasicConstraintsExtension(certificateAuthority: true, hasPathLengthConstraint: true, pathLengthConstraint: 0, critical: true));

        var san = new SubjectAlternativeNameBuilder();
        san.AddDnsName("localhost");
        san.AddIpAddress(IPAddress.Loopback);       // 127.0.0.1
        san.AddIpAddress(IPAddress.IPv6Loopback);   // ::1
        var lanIp = PrimaryLocalIp();
        if (lanIp is not null)
            san.AddIpAddress(lanIp);
        req.CertificateExtensions.Add(san.Build());

        var cert = req.CreateSelfSigned(DateTimeOffset.UtcNow.AddDays(-1), DateTimeOffset.UtcNow.AddYears(10));

        // Export/re-import so Kestrel gets a cert with an exportable private key.
        var pfx = cert.Export(X509ContentType.Pfx);
        return X509CertificateLoader.LoadPkcs12(pfx, null);
    }

    private static IPAddress? PrimaryLocalIp()
    {
        try
        {
            using var socket = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
            socket.Connect("8.8.8.8", 80);
            if (socket.LocalEndPoint is IPEndPoint ep && !IPAddress.IsLoopback(ep.Address))
                return ep.Address;
        }
        catch
        {
            // no route — fine, cert still valid for loopback
        }
        return null;
    }
}
