import { useState, useMemo, memo } from 'react';
import { rampTextClass } from '../lib/severity';
import { useApiLogStore, type ApiLogEntry } from '../stores/apiLogStore';
import { useShallow } from 'zustand/react/shallow';
import { formatMsCompact as formatMs } from '../lib/format';

type Tab = 'api' | 'render';

/** Wire bytes, in the units an operator reads at a glance. */
function formatBytes(bytes: number | null): string {
  if (bytes === null) return '—';
  if (bytes === 0) return 'from cache';
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb < 100 ? kb.toFixed(1) : Math.round(kb)} KB`;
  const mb = kb / 1024;
  return `${mb < 100 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

/**
 * Time the body was actually arriving — `responseEnd - responseStart`.
 *
 * Deliberately NOT bytes ÷ network time. The network leg is dominated by
 * round-trip latency, so that division yields a figure in bandwidth units that
 * is not bandwidth: on this API a 234 B response spends ~0.09 ms transferring
 * out of a ~40 ms leg, which would print as "48 kbps" and read as a slow link.
 * This column is a measurement, so it cannot mislead that way.
 */
function formatTransfer(ms: number | null): string {
  if (ms === null) return '—';
  if (ms < 1) return `${ms.toFixed(2)}ms`;
  if (ms < 100) return `${ms.toFixed(1)}ms`;
  return `${Math.round(ms)}ms`;
}

/** Amber once transfer is a real share of the network leg — that is the moment
 * payload size, rather than latency, is what there is to fix. */
function transferClass(entry: ApiLogEntry): string {
  if (entry.transferMs === null || entry.networkMs === null || entry.networkMs <= 0) {
    return 'text-faint';
  }
  return entry.transferMs / entry.networkMs >= 0.25 ? 'text-yellow-400' : 'text-faint';
}

/**
 * Hover detail: the split the columns cannot show, in two lines at most.
 *
 * States proportions rather than a classification — no size threshold is
 * asserted, because any threshold would be arbitrary and would be wrong at the
 * boundary. The reader is shown how much of the leg was transfer and can draw
 * the only conclusion available from it.
 */
function requestDetail(entry: ApiLogEntry): string {
  if (entry.bytes === null) {
    return 'size unknown — no Content-Length and no resource-timing entry';
  }
  if (entry.bytes === 0) {
    return 'served from cache — the network time is connection overhead, not transfer';
  }

  const size = formatBytes(entry.bytes);
  if (entry.transferMs === null || entry.networkMs === null || entry.networkMs <= 0) {
    return `${size} downloaded`;
  }

  const share = (entry.transferMs / entry.networkMs) * 100;
  return `${size} — ${formatTransfer(entry.transferMs)} of the ${formatMs(entry.networkMs)} network leg was transfer`
    + `\n${share < 1 ? '<1' : share.toFixed(0)}% transfer, the rest is round trips, TLS and queueing`;
}

function timingBar(entry: ApiLogEntry) {
  if (entry.serverMs === null) return null;
  const total = entry.totalMs || 1;
  const serverPct = Math.min(100, (entry.serverMs / total) * 100);
  const networkPct = 100 - serverPct;
  return (
    <div
      className="flex h-1.5 rounded-full overflow-hidden bg-gray-800 w-20"
      title={`Server: ${formatMs(entry.serverMs)} | Network: ${formatMs(entry.networkMs)} | ${formatBytes(entry.bytes)} (${formatTransfer(entry.transferMs)} transfer)`}
    >
      <div className="bg-cyan-500" style={{ width: `${serverPct}%` }} />
      <div className="bg-purple-500" style={{ width: `${networkPct}%` }} />
    </div>
  );
}

function statusColor(status: number): string {
  if (status === 0) return 'text-gray-400';
  if (status < 300) return 'text-green-400';
  if (status < 400) return 'text-yellow-400';
  return 'text-red-400';
}

// Latency on the shared cyan magnitude ramp (severity-v2); HTTP status
// stays green/yellow/red above — that is a STATUS, not a magnitude.
function speedIndicator(totalMs: number): string {
  return rampTextClass(totalMs, { mid: 50, high: 200, breach: 500 });
}

function renderSpeedColor(ms: number): string {
  // 16ms = 1 frame at 60fps; 50ms = 3 frames; 100ms+ = jank (breach).
  return rampTextClass(ms, { mid: 16, high: 50, breach: 100 });
}

export const ApiLogPanel = memo(function ApiLogPanel() {
  const { entries, renderEntries, enabled, clear, toggle } = useApiLogStore(
    useShallow(s => ({
      entries: s.entries,
      renderEntries: s.renderEntries,
      enabled: s.enabled,
      clear: s.clear,
      toggle: s.toggle,
    })),
  );
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<Tab>('api');
  const [filter, setFilter] = useState('');
  const [showSlow, setShowSlow] = useState(false);
  const [hidePoll, setHidePoll] = useState(false);

  const filteredApi = useMemo(() => {
    const q = filter.toLowerCase();
    return entries.filter(e => {
      if (q && !e.path.toLowerCase().includes(q)) return false;
      if (showSlow && e.totalMs < 200) return false;
      if (hidePoll && e.source === 'poll') return false;
      return true;
    });
  }, [entries, filter, showSlow, hidePoll]);

  const filteredRender = useMemo(() => {
    const q = filter.toLowerCase();
    return renderEntries.filter(e => {
      if (q && !e.component.toLowerCase().includes(q) && !e.trigger.toLowerCase().includes(q)) return false;
      if (showSlow && e.renderMs < 16) return false;
      return true;
    });
  }, [renderEntries, filter, showSlow]);

  // API stats
  const avgTotal = entries.length > 0 ? entries.reduce((s, e) => s + e.totalMs, 0) / entries.length : 0;
  const avgServer = entries.filter(e => e.serverMs !== null).length > 0
    ? entries.filter(e => e.serverMs !== null).reduce((s, e) => s + (e.serverMs ?? 0), 0) / entries.filter(e => e.serverMs !== null).length
    : 0;
  const errorCount = entries.filter(e => e.status >= 400 || e.error).length;

  // Render stats
  const avgRender = renderEntries.length > 0 ? renderEntries.reduce((s, e) => s + e.renderMs, 0) / renderEntries.length : 0;
  const slowRenders = renderEntries.filter(e => e.renderMs > 16).length;

  if (!open) {
    return (
      <button
        onClick={() => setOpen(true)}
        // z-30 keeps this below dialogs and navigation. The compact phone
        // trigger preserves a 44px touch target without masking a full row of
        // page content; the shell's pb-16 keeps final actions scrollable above it.
        className="fixed top-3 right-3 sm:top-auto sm:bottom-4 sm:right-4 z-30 min-h-11 min-w-11 sm:min-h-0 sm:min-w-0 bg-gray-900 border border-gray-700 rounded-lg px-2 sm:px-3 py-1.5 text-xs text-gray-400 hover:text-cyan-400 hover:border-cyan-500/30 transition-colors flex items-center justify-center gap-2"
        title="Performance Log"
        aria-label="Open performance log"
      >
        <span className="sm:hidden" aria-hidden="true">perf</span>
        <span className="hidden sm:contents" aria-hidden="true">
          <span>{entries.filter(e => e.source === 'user').length}</span>
          <span>user</span>
          <span className="text-faint">+{entries.filter(e => e.source === 'poll').length}</span>
          <span className="text-faint">poll</span>
          {entries.length > 0 && (
            <>
              <span className="text-faint">|</span>
              <span className={speedIndicator(avgTotal)}>{formatMs(avgTotal)} avg</span>
            </>
          )}
          {renderEntries.length > 0 && (
            <>
              <span className="text-faint">|</span>
              <span className={renderSpeedColor(avgRender)}>{formatMs(avgRender)} render</span>
            </>
          )}
          {slowRenders > 0 && (
            <>
              <span className="text-faint">|</span>
              <span className="text-orange-400">{slowRenders} slow</span>
            </>
          )}
          {errorCount > 0 && (
            <>
              <span className="text-faint">|</span>
              <span className="text-red-400">{errorCount} err</span>
            </>
          )}
        </span>
      </button>
    );
  }

  return (
    <div
      className="fixed bottom-0 right-0 z-30 min-w-0 max-w-full w-full md:w-[720px] lg:w-[900px] md:max-w-[calc(100vw-13rem)] max-h-[80dvh] md:max-h-[60vh] bg-[var(--bg-surface)] border-t border-l border-gray-700 rounded-tl-lg flex flex-col"
      role="region"
      aria-label="Performance log"
    >
      {/* Header */}
      <div className="flex flex-wrap items-start justify-between gap-2 px-3 py-2 border-b border-gray-800 flex-shrink-0">
        <div className="min-w-0 flex flex-wrap items-center gap-2">
          {/* Tabs */}
          <button
            onClick={() => setTab('api')}
            className={`px-2 py-0.5 text-xs rounded border font-bold tracking-wider ${tab === 'api' ? 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5' : 'border-gray-800 text-gray-400'}`}
          >
            API ({entries.length})
          </button>
          <button
            onClick={() => setTab('render')}
            className={`px-2 py-0.5 text-xs rounded border font-bold tracking-wider ${tab === 'render' ? 'border-green-500/30 text-green-400 bg-green-500/5' : 'border-gray-800 text-gray-400'}`}
          >
            RENDER ({renderEntries.length})
          </button>

          {/* Summary stats */}
          {tab === 'api' && entries.length > 0 && (
            <div className="flex items-center gap-2 text-xs ml-2">
              <span className="text-gray-400">avg:</span>
              <span className="text-cyan-400">{formatMs(avgServer)}</span>
              <span className="text-faint">srv</span>
              <span className="text-purple-400">{formatMs(avgTotal - avgServer)}</span>
              <span className="text-faint">net</span>
              <span className={`${speedIndicator(avgTotal)}`}>{formatMs(avgTotal)}</span>
              <span className="text-faint">total</span>
            </div>
          )}
          {tab === 'render' && renderEntries.length > 0 && (
            <div className="flex items-center gap-2 text-xs ml-2">
              <span className="text-gray-400">avg:</span>
              <span className={`${renderSpeedColor(avgRender)}`}>{formatMs(avgRender)}</span>
              <span className="text-faint">render</span>
              {slowRenders > 0 && (
                <>
                  <span className="text-faint">|</span>
                  <span className="text-orange-400">{slowRenders} &gt;16ms</span>
                </>
              )}
            </div>
          )}
        </div>
        <div className="flex items-center gap-1">
          <button onClick={toggle} className={`px-2 py-0.5 text-xs rounded border ${enabled ? 'border-green-500/30 text-green-400' : 'border-gray-700 text-gray-400'}`}>
            {enabled ? 'ON' : 'OFF'}
          </button>
          <button onClick={clear} className="px-2 py-0.5 text-xs text-gray-400 hover:text-gray-300 border border-gray-700 rounded">
            Clear
          </button>
          <button onClick={() => setOpen(false)} className="px-2 py-0.5 text-gray-400 hover:text-gray-300 text-sm" aria-label="Close performance log">&times;</button>
        </div>
      </div>

      {/* Filters */}
      <div className="flex flex-wrap items-center gap-2 px-3 py-1.5 border-b border-gray-800/50 flex-shrink-0">
        <input
          type="search"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={tab === 'api' ? 'Filter by path...' : 'Filter by component...'}
          className="min-w-0 bg-transparent border border-gray-800 rounded px-2 py-0.5 text-xs text-gray-300 w-full sm:w-40 focus:outline-none focus:border-cyan-500 placeholder:text-gray-700"
        />
        <button
          onClick={() => setShowSlow(!showSlow)}
          className={`px-2 py-0.5 text-xs rounded border ${showSlow ? 'border-orange-500/30 text-orange-400 bg-orange-500/5' : 'border-gray-700 text-gray-400'}`}
        >
          {tab === 'api' ? 'Slow (\u003E200ms)' : 'Janky (\u003E16ms)'}
        </button>
        {tab === 'api' && (
          <button
            onClick={() => setHidePoll(!hidePoll)}
            className={`px-2 py-0.5 text-xs rounded border ${hidePoll ? 'border-yellow-500/30 text-yellow-400 bg-yellow-500/5' : 'border-gray-700 text-gray-400'}`}
          >
            Hide polling
          </button>
        )}
        {tab === 'api' && (
          <div className="ml-auto flex items-center gap-2 text-xs text-faint">
            <span className="flex items-center gap-1"><span className="w-2 h-1.5 bg-cyan-500 rounded-sm" /> server</span>
            <span className="flex items-center gap-1"><span className="w-2 h-1.5 bg-purple-500 rounded-sm" /> network</span>
          </div>
        )}
        {tab === 'render' && (
          <div className="ml-auto flex items-center gap-2 text-xs text-faint">
            <span className="flex items-center gap-1"><span className="text-green-400">&lt;16ms</span> smooth</span>
            <span className="flex items-center gap-1"><span className="text-orange-400">&gt;16ms</span> jank</span>
          </div>
        )}
      </div>

      {/* Log entries */}
      <div className="overflow-auto flex-1 text-xs">
        {tab === 'api' && (
          filteredApi.length === 0 ? (
            <div className="px-3 py-8 text-center text-faint text-xs">
              {entries.length === 0 ? 'No API calls recorded yet' : 'No entries match filter'}
            </div>
          ) : (
            <table className="w-full whitespace-nowrap">
              <thead>
                <tr className="text-faint text-left border-b border-gray-800/50 sticky top-0 bg-[var(--bg-surface)]">
                  <th className="px-2 py-1 font-normal">Time</th>
                  <th className="px-2 py-1 font-normal w-12">Method</th>
                  <th className="px-2 py-1 font-normal">Path</th>
                  <th className="px-2 py-1 font-normal w-10">Status</th>
                  <th className="px-2 py-1 font-normal w-16 text-right">Total</th>
                  <th className="px-2 py-1 font-normal w-16 text-right">Server</th>
                  <th className="px-2 py-1 font-normal w-16 text-right">Network</th>
                  <th className="px-2 py-1 font-normal w-20 text-right" title="Bytes received — wire size, compressed if the response was compressed. 'cache' means it never left the browser.">Size</th>
                  <th className="px-2 py-1 font-normal w-16 text-right" title="Time the body was actually arriving (responseEnd − responseStart). This is measured, not bytes divided by the network leg — that leg is mostly round-trip latency.">Transfer</th>
                  <th className="px-2 py-1 font-normal w-20">Breakdown</th>
                </tr>
              </thead>
              <tbody>
                {filteredApi.map((e) => (
                  <tr
                    key={e.id}
                    title={requestDetail(e)}
                    className={`border-b border-gray-800/30 hover:bg-gray-800/20 ${e.error ? 'bg-red-500/5' : ''}`}
                  >
                    <td className="px-2 py-1 text-faint whitespace-nowrap">
                      {new Date(e.timestamp).toLocaleTimeString([], { hour12: false })}
                    </td>
                    <td className="px-2 py-1 text-gray-400">
                      <span>{e.method}</span>
                      {e.source === 'poll' && <span className="ml-1 text-xs text-faint" title="Background polling">&#x21BB;</span>}
                    </td>
                    <td className="px-2 py-1 text-gray-300 truncate max-w-[200px]" title={e.path}>{e.path}</td>
                    <td className={`px-2 py-1 ${statusColor(e.status)}`}>{e.status || '-'}</td>
                    <td className={`px-2 py-1 text-right ${speedIndicator(e.totalMs)}`}>{formatMs(e.totalMs)}</td>
                    <td className="px-2 py-1 text-right text-cyan-400">{formatMs(e.serverMs)}</td>
                    <td className="px-2 py-1 text-right text-purple-400">{formatMs(e.networkMs)}</td>
                    <td className="px-2 py-1 text-right text-gray-400">{formatBytes(e.bytes)}</td>
                    <td className={`px-2 py-1 text-right ${transferClass(e)}`}>{formatTransfer(e.transferMs)}</td>
                    <td className="px-2 py-1">{timingBar(e)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )
        )}

        {tab === 'render' && (
          filteredRender.length === 0 ? (
            <div className="px-3 py-8 text-center text-faint text-xs">
              {renderEntries.length === 0 ? 'No render events recorded yet — interact with filters to see data' : 'No entries match filter'}
            </div>
          ) : (
            <table className="w-full whitespace-nowrap">
              <thead>
                <tr className="text-faint text-left border-b border-gray-800/50 sticky top-0 bg-[var(--bg-surface)]">
                  <th className="px-2 py-1 font-normal">Time</th>
                  <th className="px-2 py-1 font-normal">Component</th>
                  <th className="px-2 py-1 font-normal">Trigger</th>
                  <th className="px-2 py-1 font-normal w-14 text-right">Items</th>
                  <th className="px-2 py-1 font-normal w-20 text-right">Render</th>
                  <th className="px-2 py-1 font-normal w-24">Visual</th>
                </tr>
              </thead>
              <tbody>
                {filteredRender.map((e) => {
                  const frames = e.renderMs / 16.67;
                  const barWidth = Math.min(100, frames * 10);
                  return (
                    <tr key={e.id} className={`border-b border-gray-800/30 hover:bg-gray-800/20 ${e.renderMs > 100 ? 'bg-red-500/5' : e.renderMs > 16 ? 'bg-orange-500/5' : ''}`}>
                      <td className="px-2 py-1 text-faint">{new Date(e.timestamp).toLocaleTimeString()}</td>
                      <td className="px-2 py-1 text-gray-300">{e.component}</td>
                      <td className="px-2 py-1 text-gray-400">{e.trigger}</td>
                      <td className="px-2 py-1 text-right text-gray-400">{e.itemCount ?? '-'}</td>
                      <td className={`px-2 py-1 text-right ${renderSpeedColor(e.renderMs)}`}>{formatMs(e.renderMs)}</td>
                      <td className="px-2 py-1">
                        <div className="flex items-center gap-1">
                          <div className="flex h-1.5 rounded-full overflow-hidden bg-gray-800 w-16">
                            <div
                              className={`${e.renderMs > 100 ? 'bg-red-500' : e.renderMs > 16 ? 'bg-orange-400' : 'bg-green-400'}`}
                              style={{ width: `${barWidth}%` }}
                            />
                          </div>
                          <span className="text-xs text-faint">{frames.toFixed(1)}f</span>
                        </div>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )
        )}
      </div>
    </div>
  );
});
