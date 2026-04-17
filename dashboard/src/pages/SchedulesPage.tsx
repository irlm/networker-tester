import { useState, useCallback, useMemo, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import { stableSet } from '../lib/stableUpdate';
import type { TestSchedule } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { useToast } from '../hooks/useToast';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

const SCHEDULE_STATUS_OPTIONS = ['all', 'active', 'paused'] as const;

function formatCron(expr: string): { label: string; raw: string } {
  const presets: Record<string, string> = {
    '0 * * * * *': 'Every minute',
    '0 */5 * * * *': 'Every 5 min',
    '0 */15 * * * *': 'Every 15 min',
    '0 */30 * * * *': 'Every 30 min',
    '0 0 * * * *': 'Hourly',
    '0 0 */6 * * *': 'Every 6h',
    '0 0 */12 * * *': 'Every 12h',
    '0 0 0 * * *': 'Daily midnight',
    '0 0 9 * * *': 'Daily 9:00',
    '0 0 0 * * 1': 'Weekly Mon',
    '0 0 0 * * Mon': 'Weekly Mon',
  };
  const label = presets[expr];
  if (label) return { label, raw: expr };
  const parts = expr.split(/\s+/);
  if (parts.length >= 6) {
    const [sec, min, hour] = parts;
    if (sec === '0' && min === '0' && hour.startsWith('*/'))
      return { label: `Every ${hour.slice(2)}h`, raw: expr };
    if (sec === '0' && min.startsWith('*/') && hour === '*')
      return { label: `Every ${min.slice(2)} min`, raw: expr };
  }
  return { label: 'Custom', raw: expr };
}

function timeAgo(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ago`;
  return `${Math.floor(hrs / 24)}d ago`;
}

function scheduleStatus(s: TestSchedule): { badge: string; label: string; detail: string; detailColor: string } {
  if (!s.enabled) {
    return { badge: 'offline', label: 'paused', detail: '', detailColor: '' };
  }
  if (s.next_fire_at) {
    const diff = new Date(s.next_fire_at).getTime() - Date.now();
    if (diff < 0) {
      const ago = Math.floor(-diff / 60000);
      const detail = ago < 1 ? 'due now' : ago < 60 ? `${ago}m overdue` : `${Math.floor(ago / 60)}h overdue`;
      return { badge: 'busy', label: 'overdue', detail, detailColor: 'text-yellow-400/70' };
    }
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return { badge: 'online', label: 'active', detail: 'running now', detailColor: 'text-green-400/70' };
    if (mins < 60) return { badge: 'online', label: 'active', detail: `next in ${mins}m`, detailColor: 'text-gray-500' };
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return { badge: 'online', label: 'active', detail: `next in ${hrs}h`, detailColor: 'text-gray-500' };
    return { badge: 'online', label: 'active', detail: `next in ${Math.floor(hrs / 24)}d`, detailColor: 'text-gray-600' };
  }
  return { badge: 'online', label: 'active', detail: '', detailColor: '' };
}

export function SchedulesPage() {
  const { projectId, isOperator } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [schedules, setSchedules] = useState<TestSchedule[]>([]);
  const [loading, setLoading] = useState(true);
  const [toggling, setToggling] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const addToast = useToast();
  const schedulesFingerprint = useRef('');

  const schedStatusFilter = searchParams.get('status') || 'all';
  const nameSearch = searchParams.get('name') || '';

  const setFilter = useCallback((key: string, value: string) => {
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (!value || value === 'all') next.delete(key);
      else next.set(key, value);
      return next;
    }, { replace: true });
  }, [setSearchParams]);

  const clearAllFilters = useCallback(() => setSearchParams({}, { replace: true }), [setSearchParams]);

  usePageTitle('Schedules');

  const refresh = useCallback(() => {
    if (!projectId) return;
    api.listTestSchedules(projectId)
      .then(s => { stableSet(setSchedules, s, schedulesFingerprint); setLoading(false); })
      .catch(() => { addToast('error', 'Failed to load schedules'); setLoading(false); });
  }, [addToast, projectId]);

  usePolling(refresh, 10000);

  const handleToggle = async (id: string, currentEnabled: boolean) => {
    setToggling(id);
    try {
      await api.updateTestSchedule(id, { enabled: !currentEnabled });
      addToast('success', `Schedule ${!currentEnabled ? 'enabled' : 'paused'}`);
      refresh();
    } catch {
      addToast('error', 'Failed to toggle schedule');
    } finally {
      setToggling(null);
    }
  };

  const handleTrigger = async (id: string, name: string) => {
    try {
      const run = await api.triggerTestSchedule(id);
      addToast('success', `Run started from "${name}" (${run.id.slice(0, 8)})`);
      refresh();
    } catch {
      addToast('error', 'Failed to run test');
    }
  };

  const handleDelete = async (id: string) => {
    if (confirmDelete !== id) {
      setConfirmDelete(id);
      setTimeout(() => setConfirmDelete(prev => prev === id ? null : prev), 3000);
      return;
    }
    setConfirmDelete(null);
    try {
      await api.deleteTestSchedule(id);
      addToast('success', 'Schedule deleted');
      refresh();
    } catch {
      addToast('error', 'Failed to delete schedule');
    }
  };

  const enabledCount = schedules.filter(s => s.enabled).length;

  // Client-side filtering
  const filteredSchedules = useMemo(() => {
    let list = schedules;
    if (nameSearch.trim()) {
      const q = nameSearch.toLowerCase();
      list = list.filter(s =>
        (s.config_name || '').toLowerCase().includes(q)
      );
    }
    if (schedStatusFilter === 'active') list = list.filter(s => s.enabled);
    else if (schedStatusFilter === 'paused') list = list.filter(s => !s.enabled);
    return list;
  }, [schedules, schedStatusFilter, nameSearch]);

  const schedFilterCount = [nameSearch, schedStatusFilter !== 'all'].filter(Boolean).length;

  const computedSchedules = useMemo(() =>
    filteredSchedules.map(s => ({
      ...s,
      _cron: formatCron(s.cron_expr),
      _status: scheduleStatus(s),
      _name: s.config_name || 'Unnamed',
      _isPaused: !s.enabled,
    })),
    [filteredSchedules],
  );

  if (loading && schedules.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <div className="flex items-center justify-between mb-6">
          <h2 className="text-xl font-bold text-gray-100">Schedules</h2>
          <div className="h-7 w-28 rounded bg-gray-800 motion-safe:animate-pulse" />
        </div>
        <div className="hidden md:block table-container">
          <div className="bg-[var(--bg-surface)] px-4 py-2.5 border-b border-gray-800/50">
            <div className="flex gap-8">
              {[96, 80, 64, 80, 56, 40].map((w, i) => (
                <div key={i} className="h-3 rounded bg-gray-800/60 motion-safe:animate-pulse" style={{ width: w }} />
              ))}
            </div>
          </div>
          {[1, 2, 3].map(i => (
            <div key={i} className="px-4 py-3 border-b border-gray-800/30 flex gap-8">
              {[96, 80, 64, 80, 56, 40].map((w, j) => (
                <div key={j} className="h-3 rounded bg-gray-800/40 motion-safe:animate-pulse" style={{ width: w }} />
              ))}
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 md:p-6">
      <div className="flex items-center justify-between mb-4 md:mb-6">
        <div className="flex items-center gap-3 min-w-0">
          <h2 className="text-lg md:text-xl font-bold text-gray-100">Schedules</h2>
          {schedules.length > 0 && (
            <span className="text-xs text-gray-600 hidden sm:inline">
              <span className="text-green-400">{enabledCount}</span> active
              {enabledCount !== schedules.length && (
                <> · {schedules.length} total</>
              )}
            </span>
          )}
        </div>
      </div>

      <FilterBar
        activeCount={schedFilterCount}
        onClearAll={clearAllFilters}
        chips={
          <>
            {nameSearch && <FilterChip label="Search" value={nameSearch} onClear={() => setFilter('name', '')} />}
            {schedStatusFilter !== 'all' && (
              <FilterChip label="Status" value={schedStatusFilter} onClear={() => setFilter('status', 'all')} />
            )}
          </>
        }
      >
        <input
          type="search"
          value={nameSearch}
          onChange={(e) => setFilter('name', e.target.value)}
          placeholder="Search schedules..."
          aria-label="Search schedules by name"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-300 w-40 md:w-48 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
        />
        <select
          value={schedStatusFilter}
          onChange={(e) => setFilter('status', e.target.value)}
          aria-label="Filter by status"
          className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 md:px-3 py-1.5 text-sm text-gray-300 focus:outline-none focus:border-cyan-500"
        >
          {SCHEDULE_STATUS_OPTIONS.map(s => (
            <option key={s} value={s}>
              {s === 'all' ? 'All statuses' : s.charAt(0).toUpperCase() + s.slice(1)}
            </option>
          ))}
        </select>
      </FilterBar>

      {/* Mobile card layout */}
      <div className="md:hidden space-y-2 mt-4">
        {filteredSchedules.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">{schedFilterCount > 0 ? 'No schedules match filters' : 'No scheduled tests yet'}</p>
          </div>
        ) : (
          computedSchedules.map((s) => (
            <div
              key={s.id}
              className={`border border-gray-800 rounded p-3 ${s._isPaused ? 'opacity-60' : ''}`}
            >
              <div className="flex items-start justify-between gap-2 mb-2">
                <div className="min-w-0">
                  <p className="text-gray-200 text-sm font-medium truncate">{s._name}</p>
                  <p className="text-gray-500 text-xs font-mono">{s.test_config_id.slice(0, 8)}</p>
                </div>
                <button
                  onClick={() => handleToggle(s.id, s.enabled)}
                  disabled={toggling === s.id}
                  className={`w-9 h-5 rounded-full transition-colors relative flex-shrink-0 ${
                    toggling === s.id ? 'opacity-50' : ''
                  } ${s.enabled ? 'bg-cyan-600' : 'bg-gray-700'}`}
                >
                  <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                    s.enabled ? 'left-[18px]' : 'left-0.5'
                  }`} />
                </button>
              </div>
              <div className="flex items-center gap-3 flex-wrap">
                <StatusBadge status={s._status.badge} label={s._status.label} />
                {s._status.detail && <span className={`text-xs ${s._status.detailColor}`}>{s._status.detail}</span>}
                <span className="text-xs text-cyan-400/70">{s._cron.label}</span>
                {s.last_fired_at && <span className="text-xs text-gray-600">{timeAgo(s.last_fired_at)}</span>}
              </div>
              <div className="flex items-center gap-3 mt-2 pt-2 border-t border-gray-800/50">
                {isOperator && (
                  <button onClick={() => handleTrigger(s.id, s._name)} className="text-xs text-cyan-400 py-1">
                    Run now
                  </button>
                )}
                {isOperator && (
                  <button
                    onClick={() => handleDelete(s.id)}
                    className={`text-xs py-1 transition-colors ${confirmDelete === s.id ? 'text-red-400' : 'text-gray-600'}`}
                  >
                    {confirmDelete === s.id ? 'Click to confirm delete' : 'Delete'}
                  </button>
                )}
              </div>
            </div>
          ))
        )}
      </div>

      {/* Desktop table */}
      <div className="hidden md:block table-container mt-4">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Config</th>
              <th className="px-4 py-2.5 text-left font-medium">Frequency</th>
              <th className="px-4 py-2.5 text-left font-medium">Timezone</th>
              <th className="px-4 py-2.5 text-left font-medium">Status</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Last Fired</th>
              <th className="px-4 py-2.5 text-center font-medium w-16">On</th>
              <th className="px-4 py-2.5 text-left font-medium"></th>
            </tr>
          </thead>
          <tbody>
            {computedSchedules.map((s) => (
              <tr
                key={s.id}
                className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${s._isPaused ? 'opacity-60' : ''}`}
              >
                <td className="px-4 py-3 text-gray-200">{s._name}</td>
                <td className="px-4 py-3 text-xs" title={s._cron.raw}>
                  <span className="text-cyan-400/70">{s._cron.label}</span>
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs">{s.timezone}</td>
                <td className="px-4 py-3">
                  <div className="flex items-center gap-2">
                    <StatusBadge status={s._status.badge} label={s._status.label} />
                    {s._status.detail && <span className={`text-xs ${s._status.detailColor}`}>{s._status.detail}</span>}
                  </div>
                </td>
                <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">
                  {s.last_fired_at ? timeAgo(s.last_fired_at) : '--'}
                </td>
                <td className="px-4 py-3 text-center">
                  <button
                    onClick={() => handleToggle(s.id, s.enabled)}
                    disabled={toggling === s.id}
                    className={`w-9 h-5 rounded-full transition-colors relative inline-block ${
                      toggling === s.id ? 'opacity-50' : ''
                    } ${s.enabled ? 'bg-cyan-600' : 'bg-gray-700'}`}
                    title={s.enabled ? 'Pause' : 'Resume'}
                  >
                    <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      s.enabled ? 'left-[18px]' : 'left-0.5'
                    }`} />
                  </button>
                </td>
                <td className="px-4 py-3">
                  {isOperator && (
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => handleTrigger(s.id, s._name)}
                        className="text-xs text-cyan-400 hover:text-cyan-300"
                        title="Run now"
                      >
                        &#9654;
                      </button>
                      <button
                        onClick={() => handleDelete(s.id)}
                        className={`text-xs transition-colors ${confirmDelete === s.id ? 'text-red-400' : 'text-gray-600 hover:text-red-400'}`}
                        title={confirmDelete === s.id ? 'Click again to confirm' : 'Delete'}
                      >
                        {confirmDelete === s.id ? 'delete?' : '\u2715'}
                      </button>
                    </div>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>

        {filteredSchedules.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">{schedFilterCount > 0 ? 'No schedules match the current filters' : 'No scheduled tests yet'}</p>
            <p className="text-gray-700 text-xs mt-1">
              Schedules are created as part of the New Run wizard.
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
