using Microsoft.EntityFrameworkCore;
using Networker.Data;
using Networker.Data.Entities;

namespace Networker.ControlPlane.Provisioning;

/// <summary>
/// Input for a tester lifecycle audit event — the C# port of Rust
/// <c>TesterEventInput</c> in
/// <c>crates/networker-dashboard/src/services/vm_lifecycle_recorder.rs</c>.
/// </summary>
/// <param name="ProjectId">Owning project id.</param>
/// <param name="TesterId">Tester id (becomes <c>resource_id</c>).</param>
/// <param name="TesterName">Tester name (becomes <c>resource_name</c>).</param>
/// <param name="Cloud">Cloud provider string.</param>
/// <param name="Region">Cloud region.</param>
/// <param name="VmSize">VM size / SKU.</param>
/// <param name="VmName">Cloud VM name, if known.</param>
/// <param name="VmResourceId">Cloud resource id, if known.</param>
/// <param name="CloudConnectionId">Owning cloud connection, if any.</param>
/// <param name="EventType">One of created|started|stopped|deleted|auto_shutdown|error
/// (DB CHECK constraint <c>vm_lifecycle_event_type_valid</c>).</param>
/// <param name="EventTime">When the event occurred (UTC).</param>
/// <param name="TriggeredBy">User who triggered it, if any.</param>
/// <param name="Metadata">Optional JSON metadata (serialized string).</param>
public sealed record TesterEventInput(
    string ProjectId,
    Guid TesterId,
    string TesterName,
    string Cloud,
    string Region,
    string VmSize,
    string? VmName,
    string? VmResourceId,
    Guid? CloudConnectionId,
    string EventType,
    DateTime EventTime,
    Guid? TriggeredBy,
    string? Metadata);

/// <summary>
/// Injectable audit recorder that appends <c>vm_lifecycle</c> rows on tester
/// state changes — the C# port of Rust
/// <c>crates/networker-dashboard/src/services/vm_lifecycle_recorder.rs</c>
/// (<c>insert_tester_event</c>).
///
/// <para><b>Faithful behavior:</b> failures are <b>swallowed</b> and logged at
/// WARN — never surfaced to the caller (audit history may be incomplete, but the
/// user-facing op is unaffected). <c>resource_type</c> is hardcoded to
/// <c>'tester'</c>; <c>resource_id ← tester_id</c>, <c>resource_name ← tester_name</c>,
/// exactly as the Rust INSERT.</para>
///
/// <para><b>Call sites it SHOULD be wired into</b> (document only — those files
/// are NOT edited in this port): the tester lifecycle write path
/// (<c>TesterWriteEndpoints</c>) — on each tester power/allocation change, emit
/// the matching event: <c>created</c> (provision), <c>started</c> (start),
/// <c>stopped</c> (stop), <c>deleted</c> (delete), <c>auto_shutdown</c>
/// (auto-shutdown loop), <c>error</c> (transition to error). Inject
/// <see cref="IVmLifecycleRecorder"/> and call
/// <see cref="RecordTesterEventAsync"/> after the state write commits.</para>
/// </summary>
public interface IVmLifecycleRecorder
{
    /// <summary>Append a tester lifecycle audit row. Best-effort — never throws.</summary>
    Task RecordTesterEventAsync(TesterEventInput input, CancellationToken ct = default);
}

/// <inheritdoc cref="IVmLifecycleRecorder"/>
public sealed class VmLifecycleRecorder : IVmLifecycleRecorder
{
    private readonly NetworkerDbContext _db;
    private readonly ILogger<VmLifecycleRecorder> _logger;

    public VmLifecycleRecorder(NetworkerDbContext db, ILogger<VmLifecycleRecorder> logger)
    {
        _db = db;
        _logger = logger;
    }

    public async Task RecordTesterEventAsync(TesterEventInput input, CancellationToken ct = default)
    {
        try
        {
            var row = new VmLifecycle
            {
                EventId = Guid.NewGuid(),
                ProjectId = input.ProjectId,
                ResourceType = "tester", // hardcoded literal, matching Rust
                ResourceId = input.TesterId,
                ResourceName = input.TesterName,
                Cloud = input.Cloud,
                Region = input.Region,
                VmSize = input.VmSize,
                VmName = input.VmName,
                VmResourceId = input.VmResourceId,
                CloudConnectionId = input.CloudConnectionId,
                EventType = input.EventType,
                EventTime = input.EventTime,
                TriggeredBy = input.TriggeredBy,
                Metadata = input.Metadata,
                CreatedAt = DateTime.UtcNow,
            };

            _db.VmLifecycles.Add(row);
            await _db.SaveChangesAsync(ct).ConfigureAwait(false);
        }
        catch (Exception ex)
        {
            // Rust: WARN "failed to append vm_lifecycle event from lib-side path
            // (history incomplete, user-facing op unaffected)".
            _logger.LogWarning(ex,
                "failed to append vm_lifecycle event (history incomplete, user-facing op unaffected) " +
                "tester_id={TesterId} event_type={EventType}",
                input.TesterId, input.EventType);
        }
    }
}

/// <summary>
/// DI wiring for <see cref="IVmLifecycleRecorder"/>. Scoped (it holds the scoped
/// <see cref="NetworkerDbContext"/>). Add in <c>Program.cs</c>:
/// <code>builder.Services.AddVmLifecycleRecorder();</code>
/// </summary>
public static class VmLifecycleRecorderExtensions
{
    public static IServiceCollection AddVmLifecycleRecorder(this IServiceCollection services)
    {
        services.AddScoped<IVmLifecycleRecorder, VmLifecycleRecorder>();
        return services;
    }
}
