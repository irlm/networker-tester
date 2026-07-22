namespace Networker.ControlPlane.Realtime;

/// <summary>
/// In-process observer of <see cref="EventBus"/> publications. Registered
/// implementations are invoked synchronously from <see cref="EventBus.Publish"/>
/// AFTER the event is sequenced and buffered — so an observer sees exactly the
/// events browsers see, from every producer (dispatcher, agent processor,
/// watchdog), without those producers knowing about it.
///
/// <para><b>Contract:</b> <see cref="OnEvent"/> runs on the publisher's hot path
/// and MUST NOT block — kick real work onto a detached task. Exceptions are
/// swallowed and logged by the bus (an observer bug must never break
/// publishing).</para>
/// </summary>
public interface IDashboardEventObserver
{
    void OnEvent(DashboardEvent evt);
}
