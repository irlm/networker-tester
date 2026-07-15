using System.Net;
using System.Net.Sockets;

namespace Networker.Endpoint;

/// <summary>
/// UDP echo server, ported from the Rust <c>udp_echo.rs</c> module.
/// Echoes every received datagram back to the sender verbatim. The server does
/// not interpret the payload; it just reflects the bytes.
/// </summary>
public sealed class UdpEchoService : BackgroundService
{
    private readonly ushort _port;
    private readonly ILogger<UdpEchoService> _log;

    public UdpEchoService(ushort port, ILogger<UdpEchoService> log)
    {
        _port = port;
        _log = log;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        UdpClient socket;
        try
        {
            socket = new UdpClient(new IPEndPoint(IPAddress.Any, _port));
        }
        catch (Exception e)
        {
            _log.LogWarning("UDP echo server failed to bind on 0.0.0.0:{Port}: {Err}", _port, e.Message);
            return;
        }

        _log.LogDebug("UDP echo listening on 0.0.0.0:{Port}", _port);

        try
        {
            while (!stoppingToken.IsCancellationRequested)
            {
                try
                {
                    var result = await socket.ReceiveAsync(stoppingToken);
                    await socket.SendAsync(result.Buffer, result.Buffer.Length, result.RemoteEndPoint);
                }
                catch (OperationCanceledException)
                {
                    break;
                }
                catch (Exception e)
                {
                    _log.LogWarning("UDP echo recv error: {Err}", e.Message);
                    await Task.Delay(10, stoppingToken);
                }
            }
        }
        finally
        {
            socket.Dispose();
        }
    }
}
