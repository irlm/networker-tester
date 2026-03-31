import { useState, useCallback } from 'react';
import { api } from '../api/client';
import type { Schedule } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { CreateScheduleDialog } from '../components/CreateScheduleDialog';
import { useToast } from '../hooks/useToast';
import { usePolling } from '../hooks/usePolling';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

function formatCron(expr: string): { label: string; raw: string } {
  const presets: Record<string, string> = {
    '0 * * * * *': 'Every minute',
    '0 */2 * * * *': 'Every 2 min',
    '0 */3 * * * *': 'Every 3 min',
    '0 */5 * * * *': 'Every 5 min',
    '0 */10 * * * *': 'Every 10 min',
    '0 */15 * * * *': 'Every 15 min',
    '0 */30 * * * *': 'Every 30 min',
    '0 0 * * * *': 'Hourly',
    '0 0 */2 * * *': 'Every 2h',
    '0 0 */3 * * *': 'Every 3h',
    '0 0 */6 * * *': 'Every 6h',
    '0 0 */12 * * *': 'Every 12h',
    '0 0 0 * * *': 'Daily midnight',
    '0 0 9 * * *': 'Daily 9:00',
    '0 0 0 * * 1': 'Weekly Mon',
    '0 0 0 * * Mon': 'Weekly Mon',
    '0 0 9 * * Mon': 'Weekly Mon 9:00',
    '0 0 8,12,18 * * *': '3x daily',
    '0 0 8,17 * * *': '2x daily',
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

function scheduleStatus(s: Schedule): { badge: string; label: string; detail: string; detailColor: string } {
  if (!s.enabled) {
    return { badge: 'offline', label: 'paused', detail: '', detailColor: '' };
  }
  if (s.next_run_at) {
    const diff = new Date(s.next_run_at).getTime() - Date.now();
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
  const { projectId } = useProject();
  const [schedules, setSchedules] = useState<Schedule[]>([]);
  const [loading, setLoading] = useState(true);
  const [showCreate, setShowCreate] = useState(false);
  const [toggling, setToggling] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const addToast = useToast();

  usePageTitle('Schedules');

  const refresh = useCallback(() => {
    if (!projectId) return;
    api.getSchedules(projectId)
      .then(s => { setSchedules(s); setLoading(false); })
      .catch(() => { addToast('error', 'Failed to load schedules'); setLoading(false); });
  }, [addToast, projectId]);

  usePolling(refresh, 10000);

  const handleToggle = async (id: string) => {
    setToggling(id);
    try {
      const result = await api.toggleSchedule(projectId, id);
      addToast('success', `Schedule ${result.enabled ? 'enabled' : 'paused'}`);
      refresh();
    } catch {
      addToast('error', 'Failed to toggle schedule');
    } finally {
      setToggling(null);
    }
  };

  const handleTrigger = async (id: string, name: string) => {
    try {
      const result = await api.triggerSchedule(projectId, id);
      addToast('success', `Test started from "${name}" (${result.job_id.slice(0, 8)})`);
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
      await api.deleteSchedule(projectId, id);
      addToast('success', 'Schedule deleted');
      refresh();
    } catch {
      addToast('error', 'Failed to delete schedule');
    }
  };

  const enabledCount = schedules.filter(s => s.enabled).length;
  const overdueCount = schedules.filter(s => s.enabled && s.next_run_at && new Date(s.next_run_at).getTime() < Date.now()).length;

  if (loading && schedules.length === 0) {
    return (
      <div className="p-4 md:p-6">
        <div className="flex items-center justify-between mb-6">
          <h2 className="text-xl font-bold text-gray-100">Schedules</h2>
          <div className="h-7 w-28 rounded bg-gray-800 motion-safe:animate-pulse" />
        </div>
        <div className="space-y-3 md:hidden">
          {[1, 2, 3].map(i => (
            <div key={i} className="border border-gray-800 rounded p-4 space-y-2">
              <div className="h-4 w-40 rounded bg-gray-800/60 motion-safe:animate-pulse" />
              <div className="h-3 w-24 rounded bg-gray-800/40 motion-safe:animate-pulse" />
            </div>
          ))}
        </div>
        <div className="hidden md:block table-container">
          <div className="bg-[var(--bg-surface)] px-4 py-2.5 border-b border-gray-800/50">
            <div className="flex gap-8">
              {[96, 80, 64, 80, 56, 40, 48].map((w, i) => (
                <div key={i} className="h-3 rounded bg-gray-800/60 motion-safe:animate-pulse" style={{ width: w }} />
              ))}
            </div>
          </div>
          {[1, 2, 3].map(i => (
            <div key={i} className="px-4 py-3 border-b border-gray-800/30 flex gap-8">
              {[96, 80, 64, 80, 56, 40, 48].map((w, j) => (
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
              {overdueCount > 0 && (
                <> · <span className="text-yellow-400">{overdueCount}</span> overdue</>
              )}
              {enabledCount !== schedules.length && (
                <> · {schedules.length} total</>
              )}
            </span>
          )}
        </div>
        <button
          onClick={() => setShowCreate(true)}
          className="bg-cyan-600 hover:bg-cyan-500 text-white px-3 md:px-4 py-1.5 rounded text-sm transition-colors flex-shrink-0"
        >
          New Schedule
        </button>
      </div>

      {/* ── Mobile card layout (< md) ── */}
      <div className="md:hidden space-y-2">
        {schedules.length === 0 ? (
          <div className="border border-gray-800 rounded p-8 text-center">
            <p className="text-gray-500 text-sm">No scheduled tests yet</p>
            <button onClick={() => setShowCreate(true)} className="text-xs text-cyan-400 mt-2">
              Create a schedule
            </button>
          </div>
        ) : (
          schedules.map((s) => {
            const config = s.config;
            const target = config?.target || '—';
            const cron = formatCron(s.cron_expr);
            const status = scheduleStatus(s);
            const scheduleName = s.name || 'Unnamed schedule';
            const isPaused = !s.enabled;
            let targetShort = target;
            try { targetShort = new URL(target).host; } catch { /* keep */ }

            return (
              <div
                key={s.schedule_id}
                className={`border border-gray-800 rounded p-3 ${isPaused ? 'opacity-60' : ''} ${
                  status.label === 'overdue' ? 'border-l-2 border-l-yellow-500/50' : ''
                }`}
              >
                <div className="flex items-start justify-between gap-2 mb-2">
                  <div className="min-w-0">
                    <p className="text-gray-200 text-sm font-medium truncate">
                      {scheduleName}
                      {s.benchmark_config_id && (
                        <span className="ml-2 text-xs text-purple-400 bg-purple-500/10 px-1.5 py-0.5 rounded">benchmark</span>
                      )}
                    </p>
                    <p className="text-gray-500 text-xs font-mono truncate">{s.benchmark_config_id ? 'benchmark schedule' : targetShort}</p>
                  </div>
                  <button
                    onClick={() => handleToggle(s.schedule_id)}
                    disabled={toggling === s.schedule_id}
                    className={`w-9 h-5 rounded-full transition-colors relative flex-shrink-0 ${
                      toggling === s.schedule_id ? 'opacity-50' : ''
                    } ${s.enabled ? 'bg-cyan-600' : 'bg-gray-700'}`}
                  >
                    <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                      s.enabled ? 'left-[18px]' : 'left-0.5'
                    }`} />
                  </button>
                </div>
                <div className="flex items-center gap-3 flex-wrap">
                  <StatusBadge status={status.badge} label={status.label} />
                  {status.detail && (
                    <span className={`text-xs ${status.detailColor}`}>{status.detail}</span>
                  )}
                  <span className="text-xs text-cyan-400/70">{cron.label}</span>
                  {s.last_run_at && (
                    <span className="text-xs text-gray-600">{timeAgo(s.last_run_at)}</span>
                  )}
                </div>
                <div className="flex items-center gap-3 mt-2 pt-2 border-t border-gray-800/50">
                  <button
                    onClick={() => handleTrigger(s.schedule_id, scheduleName)}
                    className="text-xs text-cyan-400 py-1"
                  >
                    ▶ Run now
                  </button>
                  <button
                    onClick={() => handleDelete(s.schedule_id)}
                    className={`text-xs py-1 transition-colors ${
                      confirmDelete === s.schedule_id ? 'text-red-400' : 'text-gray-600'
                    }`}
                  >
                    {confirmDelete === s.schedule_id ? 'Click to confirm delete' : '✕ Delete'}
                  </button>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* ── Desktop/iPad table layout (≥ md) ── */}
      <div className="hidden md:block table-container">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-800/50 text-gray-500 text-xs bg-[var(--bg-surface)]">
              <th className="px-4 py-2.5 text-left font-medium">Name</th>
              <th className="px-4 py-2.5 text-left font-medium">Target</th>
              <th className="px-4 py-2.5 text-left font-medium">Frequency</th>
              <th className="px-4 py-2.5 text-left font-medium">Status</th>
              <th className="px-4 py-2.5 text-left font-medium hidden lg:table-cell">Last Run</th>
              <th className="px-4 py-2.5 text-center font-medium w-16">On</th>
              <th className="px-4 py-2.5 text-left font-medium"></th>
            </tr>
          </thead>
          <tbody>
            {schedules.map((s) => {
              const config = s.config;
              const target = config?.target || '—';
              const cron = formatCron(s.cron_expr);
              const status = scheduleStatus(s);
              const scheduleName = s.name || 'Unnamed schedule';
              const isPaused = !s.enabled;
              const isOverdue = status.label === 'overdue';
              let targetShort = target;
              try { targetShort = new URL(target).host; } catch { /* keep */ }

              return (
                <tr
                  key={s.schedule_id}
                  className={`border-b border-gray-800/50 hover:bg-gray-800/20 ${
                    isOverdue ? 'bg-yellow-500/5' : ''
                  } ${isPaused ? 'opacity-60' : ''}`}
                >
                  <td className="px-4 py-3 text-gray-200">
                    {scheduleName}
                    {s.benchmark_config_id && (
                      <span className="ml-2 text-xs text-purple-400 bg-purple-500/10 px-1.5 py-0.5 rounded">benchmark</span>
                    )}
                  </td>
                  <td className="px-4 py-3 text-gray-400 font-mono text-xs truncate max-w-40" title={target}>
                    {targetShort}
                  </td>
                  <td className="px-4 py-3 text-xs" title={cron.raw}>
                    <span className="text-cyan-400/70">{cron.label}</span>
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <StatusBadge status={status.badge} label={status.label} />
                      {status.detail && (
                        <span className={`text-xs ${status.detailColor}`}>{status.detail}</span>
                      )}
                    </div>
                  </td>
                  <td className="px-4 py-3 text-gray-500 text-xs hidden lg:table-cell">
                    {s.last_run_at ? timeAgo(s.last_run_at) : '—'}
                  </td>
                  <td className="px-4 py-3 text-center">
                    <button
                      onClick={() => handleToggle(s.schedule_id)}
                      disabled={toggling === s.schedule_id}
                      className={`w-9 h-5 rounded-full transition-colors relative inline-block ${
                        toggling === s.schedule_id ? 'opacity-50' : ''
                      } ${s.enabled ? 'bg-cyan-600' : 'bg-gray-700'}`}
                      title={s.enabled ? 'Pause' : 'Resume'}
                    >
                      <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                        s.enabled ? 'left-[18px]' : 'left-0.5'
                      }`} />
                    </button>
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => handleTrigger(s.schedule_id, scheduleName)}
                        className="text-xs text-cyan-400 hover:text-cyan-300"
                        title="Run now"
                      >
                        ▶
                      </button>
                      <button
                        onClick={() => handleDelete(s.schedule_id)}
                        className={`text-xs transition-colors ${
                          confirmDelete === s.schedule_id ? 'text-red-400' : 'text-gray-600 hover:text-red-400'
                        }`}
                        title={confirmDelete === s.schedule_id ? 'Click again to confirm' : 'Delete'}
                      >
                        {confirmDelete === s.schedule_id ? 'delete?' : '✕'}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>

        {schedules.length === 0 && (
          <div className="py-10 text-center">
            <p className="text-gray-500 text-sm">No scheduled tests yet</p>
            <p className="text-gray-700 text-xs mt-1">
              <button onClick={() => setShowCreate(true)} className="text-cyan-400 hover:text-cyan-300 transition-colors">
                Create a schedule
              </button>
              {' '}to run tests automatically
            </p>
          </div>
        )}
      </div>

      {showCreate && (
        <CreateScheduleDialog
          projectId={projectId}
          onClose={() => setShowCreate(false)}
          onCreated={refresh}
        />
      )}
    </div>
  );
}
