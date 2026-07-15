using System.Buffers.Binary;
using System.Net;
using System.Net.Sockets;

namespace Networker.Endpoint;

/// <summary>
/// UDP bulk throughput server, ported from the Rust <c>udp_throughput.rs</c>.
///
/// Wire protocol (all multi-byte integers little-endian):
///
/// Control packet (12 bytes): [0..4] magic "NWKT", [4] cmd, [5..8] padding,
/// [8..12] value u32 LE.
///   0x01 CMD_DOWNLOAD (client→server, requested bytes)
///   0x02 CMD_UPLOAD   (client→server, total bytes)
///   0x04 CMD_DONE     (client→server, upload complete)
///   0x10 CMD_ACK      (server→client, ready)
///   0x11 CMD_REPORT   (server→client, bytes received)
///
/// Data packet: [0..4] seq_num u32 LE, [4..8] total_seqs u32 LE, [8..] payload.
/// </summary>
public sealed class UdpThroughputService : BackgroundService
{
    private static readonly byte[] Magic = "NWKT"u8.ToArray();
    private const byte CmdDownload = 0x01;
    private const byte CmdUpload = 0x02;
    private const byte CmdDone = 0x04;
    private const byte CmdAck = 0x10;
    private const byte CmdReport = 0x11;
    private const int CtrlLen = 12;
    private const int DataHdrLen = 8;
    private const int ChunkSize = 1400;
    private static readonly TimeSpan UploadStateTtl = TimeSpan.FromSeconds(60);

    private readonly ushort _port;
    private readonly ILogger<UdpThroughputService> _log;

    public UdpThroughputService(ushort port, ILogger<UdpThroughputService> log)
    {
        _port = port;
        _log = log;
    }

    private sealed class UploadState
    {
        public int ExpectedBytes;
        public readonly HashSet<uint> ReceivedSeqs = new();
        public long ReceivedBytes;
        public DateTime CreatedAt = DateTime.UtcNow;
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        UdpClient sock;
        try
        {
            sock = new UdpClient(new IPEndPoint(IPAddress.Any, _port));
        }
        catch (Exception e)
        {
            _log.LogWarning("Failed to bind UDP throughput socket on 0.0.0.0:{Port}: {Err}", _port, e.Message);
            return;
        }

        _log.LogInformation("UDP throughput -> 0.0.0.0:{Port}", _port);

        var uploadStates = new Dictionary<IPEndPoint, UploadState>();
        ulong pktCounter = 0;

        try
        {
            while (!stoppingToken.IsCancellationRequested)
            {
                UdpReceiveResult recv;
                try
                {
                    recv = await sock.ReceiveAsync(stoppingToken);
                }
                catch (OperationCanceledException)
                {
                    break;
                }
                catch (Exception e)
                {
                    _log.LogDebug("UDP throughput recv error: {Err}", e.Message);
                    continue;
                }

                var pkt = recv.Buffer;
                var n = pkt.Length;
                var src = recv.RemoteEndPoint;

                if (n == CtrlLen && pkt.AsSpan(0, 4).SequenceEqual(Magic))
                {
                    var cmd = pkt[4];
                    var value = (int)BinaryPrimitives.ReadUInt32LittleEndian(pkt.AsSpan(8, 4));

                    switch (cmd)
                    {
                        case CmdDownload:
                            await sock.SendAsync(MakeCtrl(CmdAck, 0), CtrlLen, src);
                            _ = Task.Run(() => SendDownloadAsync(sock, src, value), CancellationToken.None);
                            break;

                        case CmdUpload:
                            uploadStates[src] = new UploadState { ExpectedBytes = value };
                            await sock.SendAsync(MakeCtrl(CmdAck, 0), CtrlLen, src);
                            break;

                        case CmdDone:
                            if (uploadStates.Remove(src, out var doneState))
                            {
                                var report = MakeCtrl(CmdReport, (uint)doneState.ReceivedBytes);
                                await sock.SendAsync(report, CtrlLen, src);
                            }
                            break;
                    }
                }
                else if (n > DataHdrLen)
                {
                    if (uploadStates.TryGetValue(src, out var state))
                    {
                        var seq = BinaryPrimitives.ReadUInt32LittleEndian(pkt.AsSpan(0, 4));
                        var dataLen = n - DataHdrLen;
                        if (state.ReceivedSeqs.Add(seq))
                            state.ReceivedBytes += dataLen;
                    }
                }

                pktCounter++;
                if (pktCounter % 100 == 0)
                {
                    var now = DateTime.UtcNow;
                    var stale = uploadStates
                        .Where(kv => now - kv.Value.CreatedAt >= UploadStateTtl)
                        .Select(kv => kv.Key)
                        .ToList();
                    foreach (var key in stale)
                        uploadStates.Remove(key);
                }
            }
        }
        finally
        {
            sock.Dispose();
        }
    }

    private static async Task SendDownloadAsync(UdpClient sock, IPEndPoint dst, int totalBytes)
    {
        if (totalBytes == 0)
        {
            await sock.SendAsync(MakeCtrl(CmdDone, 0), CtrlLen, dst);
            return;
        }

        var totalSeqs = (uint)((totalBytes + ChunkSize - 1) / ChunkSize);
        var sentBytes = 0;

        for (uint seq = 0; seq < totalSeqs; seq++)
        {
            var payloadSize = Math.Min(totalBytes - sentBytes, ChunkSize);
            var pkt = new byte[DataHdrLen + payloadSize];
            BinaryPrimitives.WriteUInt32LittleEndian(pkt.AsSpan(0, 4), seq);
            BinaryPrimitives.WriteUInt32LittleEndian(pkt.AsSpan(4, 4), totalSeqs);
            try
            {
                await sock.SendAsync(pkt, pkt.Length, dst);
            }
            catch
            {
                break;
            }
            sentBytes += payloadSize;
        }

        await sock.SendAsync(MakeCtrl(CmdDone, (uint)totalBytes), CtrlLen, dst);
    }

    private static byte[] MakeCtrl(byte cmd, uint value)
    {
        var pkt = new byte[CtrlLen];
        Magic.CopyTo(pkt, 0);
        pkt[4] = cmd;
        BinaryPrimitives.WriteUInt32LittleEndian(pkt.AsSpan(8, 4), value);
        return pkt;
    }
}
