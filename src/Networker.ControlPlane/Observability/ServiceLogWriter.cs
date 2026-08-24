using System.Diagnostics;
using System.Threading.Channels;
using Npgsql;
using NpgsqlTypes;

namespace Networker.ControlPlane.Observability;

/// <summary>
/// The batching writer behind the <c>service_log</c> sink: a bounded queue that
/// producers never block on, drained by one background loop that COPYs batches
/// into Postgres.
///
/// <para><b>Three rules this type exists to keep.</b></para>
/// <list type="number">
///   <item><b>A log call must never block.</b> Producers do a non-blocking
///     <c>TryWrite</c>; when the queue is full the entry is dropped and counted.
///     A control plane that stalls its request threads because the log database
///     is slow has turned observability into an outage.</item>
///   <item><b>A log call must never throw.</b> Everything here is caught; the
///     worst outcome is a dropped row and a counter.</item>
///   <item><b>The writer must never log through <c>ILogger</c>.</b> Its own
///     diagnostics would re-enter the sink and, on a database failure, produce
///     an error per failed flush forever. It writes to stderr instead, which is
///     where the control plane's logs already go.</item>
/// </list>
/// </summary>
public sealed class ServiceLogWriter : IAsyncDisposable
{
    private readonly Channel<ServiceLogEntry> _queue;
    private readonly ServiceLogOptions _options;
    private readonly ServiceLogMetrics _metrics;
    private readonly NpgsqlDataSource _dataSource;
    private readonly CancellationTokenSource _stopping = new();
    private Task? _pump;

    public ServiceLogWriter(NpgsqlDataSource dataSource, ServiceLogOptions options, ServiceLogMetrics metrics)
    {
        _dataSource = dataSource;
        _options = options;
        _metrics = metrics;
        _queue = Channel.CreateBounded<ServiceLogEntry>(new BoundedChannelOptions(options.QueueCapacity)
        {
            // Drop the NEWEST rather than block or evict history: when the sink
            // is behind, the older entries already queued are the ones that
            // explain how it got there.
            FullMode = BoundedChannelFullMode.DropWrite,
            SingleReader = true,
            SingleWriter = false,
        });
    }

    /// <summary>Queue an entry. Returns false when it was dropped. Never blocks,
    /// never throws.</summary>
    public bool TryEnqueue(in ServiceLogEntry entry)
    {
        if (_queue.Writer.TryWrite(entry))
        {
            _metrics.OnEnqueued();
            return true;
        }

        _metrics.OnDropped();
        return false;
    }

    public void Start() => _pump ??= Task.Run(PumpAsync);

    private async Task PumpAsync()
    {
        var batch = new List<ServiceLogEntry>(_options.BatchSize);
        var reader = _queue.Reader;

        while (!_stopping.IsCancellationRequested)
        {
            try
            {
                // Wait for at least one entry, then take whatever else is ready
                // up to the batch size — one round trip for a burst.
                if (!await reader.WaitToReadAsync(_stopping.Token).ConfigureAwait(false))
                {
                    break;
                }

                while (batch.Count < _options.BatchSize && reader.TryRead(out var entry))
                {
                    batch.Add(entry);
                }

                if (batch.Count == 0)
                {
                    continue;
                }

                await FlushAsync(batch, CancellationToken.None).ConfigureAwait(false);
                batch.Clear();

                // Give a trickle of entries a chance to coalesce instead of one
                // round trip per line.
                if (reader.Count == 0)
                {
                    await Task.Delay(_options.FlushInterval, _stopping.Token).ConfigureAwait(false);
                }
            }
            catch (OperationCanceledException)
            {
                break;
            }
            catch (Exception ex)
            {
                // The pump itself must survive anything.
                Fail(batch.Count, ex);
                batch.Clear();
                try
                {
                    await Task.Delay(TimeSpan.FromSeconds(5), _stopping.Token).ConfigureAwait(false);
                }
                catch (OperationCanceledException)
                {
                    break;
                }
            }
        }

        // Drain what is already queued so a clean shutdown does not lose the
        // last few seconds — bounded by the batch size so it cannot hang.
        batch.Clear();
        while (batch.Count < _options.BatchSize && reader.TryRead(out var entry))
        {
            batch.Add(entry);
        }
        if (batch.Count > 0)
        {
            try
            {
                using var drain = new CancellationTokenSource(TimeSpan.FromSeconds(5));
                await FlushAsync(batch, drain.Token).ConfigureAwait(false);
            }
            catch (Exception ex)
            {
                Fail(batch.Count, ex);
            }
        }
    }

    private async Task FlushAsync(List<ServiceLogEntry> batch, CancellationToken ct)
    {
        var sw = Stopwatch.StartNew();
        try
        {
            await using var conn = await _dataSource.OpenConnectionAsync(ct).ConfigureAwait(false);
            await using var writer = await conn.BeginBinaryImportAsync(
                "COPY service_log (ts, service, level, message, config_id, project_id, trace_id, fields) FROM STDIN (FORMAT BINARY)",
                ct).ConfigureAwait(false);

            foreach (var e in batch)
            {
                await writer.StartRowAsync(ct).ConfigureAwait(false);
                await writer.WriteAsync(e.Timestamp, NpgsqlDbType.TimestampTz, ct).ConfigureAwait(false);
                await writer.WriteAsync(e.Service, NpgsqlDbType.Text, ct).ConfigureAwait(false);
                await writer.WriteAsync(e.Level, NpgsqlDbType.Smallint, ct).ConfigureAwait(false);
                await writer.WriteAsync(e.Message, NpgsqlDbType.Text, ct).ConfigureAwait(false);
                await WriteNullableAsync(writer, e.ConfigId, NpgsqlDbType.Uuid, ct).ConfigureAwait(false);
                await WriteNullableAsync(writer, e.ProjectId, NpgsqlDbType.Char, ct).ConfigureAwait(false);
                await WriteNullableAsync(writer, e.TraceId, NpgsqlDbType.Uuid, ct).ConfigureAwait(false);
                await WriteNullableAsync(writer, e.Fields, NpgsqlDbType.Jsonb, ct).ConfigureAwait(false);
            }

            await writer.CompleteAsync(ct).ConfigureAwait(false);
            _metrics.OnFlushed(batch.Count, sw.ElapsedMilliseconds);
        }
        catch (Exception ex)
        {
            Fail(batch.Count, ex);
        }
    }

    private static async Task WriteNullableAsync<T>(
        NpgsqlBinaryImporter writer, T? value, NpgsqlDbType type, CancellationToken ct)
    {
        if (value is null)
        {
            await writer.WriteNullAsync(ct).ConfigureAwait(false);
        }
        else
        {
            await writer.WriteAsync(value, type, ct).ConfigureAwait(false);
        }
    }

    /// <summary>Record a failed batch and say so ONCE per failure on stderr —
    /// never through ILogger, which would re-enter this sink.</summary>
    private void Fail(int rows, Exception ex)
    {
        if (rows > 0)
        {
            _metrics.OnFlushFailed(rows);
        }

        Console.Error.WriteLine(
            $"[service-log-sink] flush failed ({rows} row(s) dropped): {ex.GetType().Name}: {ex.Message}");
    }

    public async ValueTask DisposeAsync()
    {
        _queue.Writer.TryComplete();
        await _stopping.CancelAsync().ConfigureAwait(false);
        if (_pump is not null)
        {
            try
            {
                await _pump.WaitAsync(TimeSpan.FromSeconds(10)).ConfigureAwait(false);
            }
            catch (Exception ex) when (ex is TimeoutException or OperationCanceledException)
            {
                // Shutdown is not the place to hang on a log flush.
            }
        }
        _stopping.Dispose();
    }
}
