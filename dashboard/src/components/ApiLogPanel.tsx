import { useState, useMemo, memo } from 'react';
import { useApiLogStore, type ApiLogEntry } from '../stores/apiLogStore';
import { useShallow } from 'zustand/react/shallow';

type Tab = 'api' | 'render';

function formatMs(ms: number | null): string {
  if (ms === null) return '-';
  if (ms < 1) return '<1ms';
  return `${ms.toFixed(1)}ms`;
}

function timingBar(entry: ApiLogEntry) {
  if (entry.serverMs === null) return null;
  const total = entry.totalMs || 1;
  const serverPct = Math.min(100, (entry.serverMs / total) * 100);
  const networkPct = 100 - serverPct;
  return (
    <div className="flex h-1.5 rounded-full overflow-hidden bg-gray-800 w-20" title={`Server: ${formatMs(entry.serverMs)} | Network: ${formatMs(entry.networkMs)}`}>
      <div className="bg-cyan-500" style={{ width: `${serverPct}%` }} />
      <div className="bg-purple-500" style={{ width: `${networkPct}%` }} />
    </div>
  );
}

function statusColor(status: number): string {
  if (status === 0) return 'text-gray-500';
  if (status < 300) return 'text-green-400';
  if (status < 400) return 'text-yellow-400';
  return 'text-red-400';
}

function speedIndicator(totalMs: number): string {
  if (totalMs < 50) return 'text-green-400';
  if (totalMs < 200) return 'text-yellow-400';
  if (totalMs < 500) return 'text-orange-400';
  return 'text-red-400';
}

function renderSpeedColor(ms: number): string {
  if (ms < 16) return 'text-green-400';   // under 1 frame (60fps)
  if (ms < 50) return 'text-yellow-400';  // under 3 frames
  if (ms < 100) return 'text-orange-400';
  return 'text-red-400';                  // jank
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
        className="fixed bottom-4 right-4 z-50 bg-gray-900 border border-gray-700 rounded-lg px-3 py-1.5 text-xs text-gray-400 hover:text-cyan-400 hover:border-cyan-500/30 transition-colors shadow-lg flex items-center gap-2"
        title="Performance Log"
      >
        <span className="font-mono">{entries.filter(e => e.source === 'user').length}</span>
        <span>user</span>
        <span className="text-gray-600 font-mono">+{entries.filter(e => e.source === 'poll').length}</span>
        <span className="text-gray-600">poll</span>
        {entries.length > 0 && (
          <>
            <span className="text-gray-600">|</span>
            <span className={speedIndicator(avgTotal)}>{formatMs(avgTotal)} avg</span>
          </>
        )}
        {renderEntries.length > 0 && (
          <>
            <span className="text-gray-600">|</span>
            <span className={renderSpeedColor(avgRender)}>{formatMs(avgRender)} render</span>
          </>
        )}
        {slowRenders > 0 && (
          <>
            <span className="text-gray-600">|</span>
            <span className="text-orange-400">{slowRenders} slow</span>
          </>
        )}
        {errorCount > 0 && (
          <>
            <span className="text-gray-600">|</span>
            <span className="text-red-400">{errorCount} err</span>
          </>
        )}
      </button>
    );
  }

  return (
    <div className="fixed bottom-0 right-0 z-50 w-full md:w-[640px] lg:w-[760px] max-h-[60vh] bg-[#0d0e14] border-t border-l border-gray-700 rounded-tl-lg shadow-2xl flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-gray-800 flex-shrink-0">
        <div className="flex items-center gap-2">
          {/* Tabs */}
          <button
            onClick={() => setTab('api')}
            className={`px-2 py-0.5 text-[10px] rounded border font-bold tracking-wider ${tab === 'api' ? 'border-cyan-500/30 text-cyan-400 bg-cyan-500/5' : 'border-gray-800 text-gray-500'}`}
          >
            API ({entries.length})
          </button>
          <button
            onClick={() => setTab('render')}
            className={`px-2 py-0.5 text-[10px] rounded border font-bold tracking-wider ${tab === 'render' ? 'border-green-500/30 text-green-400 bg-green-500/5' : 'border-gray-800 text-gray-500'}`}
          >
            RENDER ({renderEntries.length})
          </button>

          {/* Summary stats */}
          {tab === 'api' && entries.length > 0 && (
            <div className="flex items-center gap-2 text-[10px] ml-2">
              <span className="text-gray-500">avg:</span>
              <span className="text-cyan-400 font-mono">{formatMs(avgServer)}</span>
              <span className="text-gray-600">srv</span>
              <span className="text-purple-400 font-mono">{formatMs(avgTotal - avgServer)}</span>
              <span className="text-gray-600">net</span>
              <span className={`font-mono ${speedIndicator(avgTotal)}`}>{formatMs(avgTotal)}</span>
              <span className="text-gray-600">total</span>
            </div>
          )}
          {tab === 'render' && renderEntries.length > 0 && (
            <div className="flex items-center gap-2 text-[10px] ml-2">
              <span className="text-gray-500">avg:</span>
              <span className={`font-mono ${renderSpeedColor(avgRender)}`}>{formatMs(avgRender)}</span>
              <span className="text-gray-600">render</span>
              {slowRenders > 0 && (
                <>
                  <span className="text-gray-600">|</span>
                  <span className="text-orange-400">{slowRenders} &gt;16ms</span>
                </>
              )}
            </div>
          )}
        </div>
        <div className="flex items-center gap-1">
          <button onClick={toggle} className={`px-2 py-0.5 text-[10px] rounded border ${enabled ? 'border-green-500/30 text-green-400' : 'border-gray-700 text-gray-500'}`}>
            {enabled ? 'ON' : 'OFF'}
          </button>
          <button onClick={clear} className="px-2 py-0.5 text-[10px] text-gray-500 hover:text-gray-300 border border-gray-700 rounded">
            Clear
          </button>
          <button onClick={() => setOpen(false)} className="px-2 py-0.5 text-gray-500 hover:text-gray-300 text-sm">&times;</button>
        </div>
      </div>

      {/* Filters */}
      <div className="flex items-center gap-2 px-3 py-1.5 border-b border-gray-800/50 flex-shrink-0">
        <input
          type="search"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={tab === 'api' ? 'Filter by path...' : 'Filter by component...'}
          className="bg-transparent border border-gray-800 rounded px-2 py-0.5 text-xs text-gray-300 w-40 focus:outline-none focus:border-cyan-500 placeholder:text-gray-700"
        />
        <button
          onClick={() => setShowSlow(!showSlow)}
          className={`px-2 py-0.5 text-[10px] rounded border ${showSlow ? 'border-orange-500/30 text-orange-400 bg-orange-500/5' : 'border-gray-700 text-gray-500'}`}
        >
          {tab === 'api' ? 'Slow (\u003E200ms)' : 'Janky (\u003E16ms)'}
        </button>
        {tab === 'api' && (
          <button
            onClick={() => setHidePoll(!hidePoll)}
            className={`px-2 py-0.5 text-[10px] rounded border ${hidePoll ? 'border-yellow-500/30 text-yellow-400 bg-yellow-500/5' : 'border-gray-700 text-gray-500'}`}
          >
            Hide polling
          </button>
        )}
        {tab === 'api' && (
          <div className="ml-auto flex items-center gap-2 text-[10px] text-gray-600">
            <span className="flex items-center gap-1"><span className="w-2 h-1.5 bg-cyan-500 rounded-sm" /> server</span>
            <span className="flex items-center gap-1"><span className="w-2 h-1.5 bg-purple-500 rounded-sm" /> network</span>
          </div>
        )}
        {tab === 'render' && (
          <div className="ml-auto flex items-center gap-2 text-[10px] text-gray-600">
            <span className="flex items-center gap-1"><span className="text-green-400">&lt;16ms</span> smooth</span>
            <span className="flex items-center gap-1"><span className="text-orange-400">&gt;16ms</span> jank</span>
          </div>
        )}
      </div>

      {/* Log entries */}
      <div className="overflow-auto flex-1 font-mono text-[11px]">
        {tab === 'api' && (
          filteredApi.length === 0 ? (
            <div className="px-3 py-8 text-center text-gray-600 text-xs">
              {entries.length === 0 ? 'No API calls recorded yet' : 'No entries match filter'}
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr className="text-gray-600 text-left border-b border-gray-800/50 sticky top-0 bg-[#0d0e14]">
                  <th className="px-2 py-1 font-normal">Time</th>
                  <th className="px-2 py-1 font-normal w-12">Method</th>
                  <th className="px-2 py-1 font-normal">Path</th>
                  <th className="px-2 py-1 font-normal w-10">Status</th>
                  <th className="px-2 py-1 font-normal w-16 text-right">Total</th>
                  <th className="px-2 py-1 font-normal w-16 text-right">Server</th>
                  <th className="px-2 py-1 font-normal w-16 text-right">Network</th>
                  <th className="px-2 py-1 font-normal w-20">Breakdown</th>
                </tr>
              </thead>
              <tbody>
                {filteredApi.map((e) => (
                  <tr key={e.id} className={`border-b border-gray-800/30 hover:bg-gray-800/20 ${e.error ? 'bg-red-500/5' : ''}`}>
                    <td className="px-2 py-1 text-gray-600">{new Date(e.timestamp).toLocaleTimeString()}</td>
                    <td className="px-2 py-1 text-gray-500">
                      <span>{e.method}</span>
                      {e.source === 'poll' && <span className="ml-1 text-[9px] text-gray-600" title="Background polling">&#x21BB;</span>}
                    </td>
                    <td className="px-2 py-1 text-gray-300 truncate max-w-[200px]" title={e.path}>{e.path}</td>
                    <td className={`px-2 py-1 ${statusColor(e.status)}`}>{e.status || '-'}</td>
                    <td className={`px-2 py-1 text-right ${speedIndicator(e.totalMs)}`}>{formatMs(e.totalMs)}</td>
                    <td className="px-2 py-1 text-right text-cyan-400">{formatMs(e.serverMs)}</td>
                    <td className="px-2 py-1 text-right text-purple-400">{formatMs(e.networkMs)}</td>
                    <td className="px-2 py-1">{timingBar(e)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )
        )}

        {tab === 'render' && (
          filteredRender.length === 0 ? (
            <div className="px-3 py-8 text-center text-gray-600 text-xs">
              {renderEntries.length === 0 ? 'No render events recorded yet — interact with filters to see data' : 'No entries match filter'}
            </div>
          ) : (
            <table className="w-full">
              <thead>
                <tr className="text-gray-600 text-left border-b border-gray-800/50 sticky top-0 bg-[#0d0e14]">
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
                      <td className="px-2 py-1 text-gray-600">{new Date(e.timestamp).toLocaleTimeString()}</td>
                      <td className="px-2 py-1 text-gray-300">{e.component}</td>
                      <td className="px-2 py-1 text-gray-400">{e.trigger}</td>
                      <td className="px-2 py-1 text-right text-gray-500">{e.itemCount ?? '-'}</td>
                      <td className={`px-2 py-1 text-right ${renderSpeedColor(e.renderMs)}`}>{formatMs(e.renderMs)}</td>
                      <td className="px-2 py-1">
                        <div className="flex items-center gap-1">
                          <div className="flex h-1.5 rounded-full overflow-hidden bg-gray-800 w-16">
                            <div
                              className={`${e.renderMs > 100 ? 'bg-red-500' : e.renderMs > 16 ? 'bg-orange-400' : 'bg-green-400'}`}
                              style={{ width: `${barWidth}%` }}
                            />
                          </div>
                          <span className="text-[9px] text-gray-600">{frames.toFixed(1)}f</span>
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
