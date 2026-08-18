import { useState, useCallback, useMemo } from 'react';
import { DataTable } from '../components/common/DataTable';
import { Link, useSearchParams } from 'react-router';
import { errorMessage } from '../api/client';
import { timeAgo } from '../lib/format';
import type { TestSchedule } from '../api/types';
import { StatusBadge } from '../components/common/StatusBadge';
import { FilterBar, FilterChip } from '../components/common/FilterBar';
import { useToast } from '../hooks/useToast';
import { StatusFooter } from '../components/common/StatusFooter';
import { Button } from '../components/common/Button';
import { buttonClassName } from '../components/common/button-styles';
import { ConfirmDialog } from '../components/common/ConfirmDialog';
import { ErrorState, LoadingState } from '../components/common/AsyncState';
import { Input, Select } from '../components/common/FormControls';
import { PageShell } from '../components/common/PageShell';
import {
  useDeleteScheduleMutation,
  useSchedulesQuery,
  useTriggerScheduleMutation,
  useUpdateScheduleMutation,
} from '../features/runs/queries';
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
    if (mins < 60) return { badge: 'online', label: 'active', detail: `next in ${mins}m`, detailColor: 'text-gray-400' };
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return { badge: 'online', label: 'active', detail: `next in ${hrs}h`, detailColor: 'text-gray-400' };
    return { badge: 'online', label: 'active', detail: `next in ${Math.floor(hrs / 24)}d`, detailColor: 'text-faint' };
  }
  return { badge: 'online', label: 'active', detail: '', detailColor: '' };
}

export function SchedulesPage() {
  const { projectId, isOperator } = useProject();
  const [searchParams, setSearchParams] = useSearchParams();
  const [paused, setPaused] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const addToast = useToast();
  const schedulesQuery = useSchedulesQuery(projectId, !paused);
  const updateSchedule = useUpdateScheduleMutation(projectId);
  const triggerSchedule = useTriggerScheduleMutation(projectId);
  const deleteSchedule = useDeleteScheduleMutation(projectId);
  const schedules = useMemo(() => schedulesQuery.data ?? [], [schedulesQuery.data]);
  const toggling = updateSchedule.isPending ? updateSchedule.variables?.scheduleId ?? null : null;

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

  const handleToggle = async (id: string, currentEnabled: boolean) => {
    try {
      await updateSchedule.mutateAsync({ scheduleId: id, enabled: !currentEnabled });
      addToast('success', `Schedule ${!currentEnabled ? 'enabled' : 'paused'}`);
    } catch {
      addToast('error', 'Failed to toggle schedule');
    }
  };

  const handleTrigger = async (id: string, name: string) => {
    try {
      const run = await triggerSchedule.mutateAsync(id);
      addToast('success', `Run started from "${name}" (${run.id.slice(0, 8)})`);
    } catch {
      addToast('error', 'Failed to run test');
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteSchedule.mutateAsync(id);
      addToast('success', 'Schedule deleted');
      setConfirmDelete(null);
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
  const scheduleToDelete = schedules.find(schedule => schedule.id === confirmDelete);

  if (schedulesQuery.isPending && schedules.length === 0) {
    return (
      <PageShell title="Schedules">
        <LoadingState label="Loading schedules…">
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
        </LoadingState>
      </PageShell>
    );
  }

  if (schedulesQuery.error && schedules.length === 0) {
    return (
      <PageShell title="Schedules">
        <ErrorState
          title="Schedules unavailable"
          message={errorMessage(schedulesQuery.error)}
          onRetry={() => { void schedulesQuery.refetch(); }}
        />
      </PageShell>
    );
  }

  return (
    <PageShell
      title="Schedules"
      action={schedules.length > 0 ? (
        <span className="text-xs text-faint">
          <span className="text-green-400">{enabledCount}</span> active
          {enabledCount !== schedules.length && <> · {schedules.length} total</>}
        </span>
      ) : undefined}
    >

      {/* No filter chrome before there is anything to filter (audit §10). */}
      {(schedules.length > 0 || schedFilterCount > 0) && (
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
        <div className="w-40 md:w-48">
          <Input
            type="search"
            value={nameSearch}
            onChange={(e) => setFilter('name', e.target.value)}
            placeholder="Search schedules..."
            aria-label="Search schedules by name"
          />
        </div>
        <div className="w-40">
          <Select
            value={schedStatusFilter}
            onChange={(e) => setFilter('status', e.target.value)}
            aria-label="Filter by status"
          >
            {SCHEDULE_STATUS_OPTIONS.map(s => (
              <option key={s} value={s}>
                {s === 'all' ? 'All statuses' : s.charAt(0).toUpperCase() + s.slice(1)}
              </option>
            ))}
          </Select>
        </div>
      </FilterBar>
      )}

      <DataTable
        className="mt-4"
        columns={[
          {
            key: 'config',
            label: 'Config',
            cellClass: 'text-gray-200 text-sm',
            render: (s) => <span className={s._isPaused ? 'opacity-60' : ''}>{s._name}</span>,
          },
          {
            key: 'frequency',
            label: 'Frequency',
            titleOf: (s) => s._cron.raw,
            render: (s) => <span className="text-cyan-400/70">{s._cron.label}</span>,
          },
          { key: 'timezone', label: 'Timezone', hideBelow: 'md', cellClass: 'text-gray-400', render: (s) => s.timezone },
          {
            key: 'status',
            label: 'Status',
            render: (s) => (
              <div className="flex items-center gap-2">
                <StatusBadge status={s._status.badge} label={s._status.label} />
                {s._status.detail && <span className={`text-xs ${s._status.detailColor}`}>{s._status.detail}</span>}
              </div>
            ),
          },
          {
            key: 'last-fired',
            label: 'Last Fired',
            hideBelow: 'lg',
            cellClass: 'text-gray-400',
            render: (s) => (s.last_fired_at ? timeAgo(s.last_fired_at) : '--'),
          },
          {
            key: 'enabled',
            label: 'On',
            render: (s) => (
              <button
                onClick={() => handleToggle(s.id, s.enabled)}
                disabled={toggling === s.id}
                role="switch"
                aria-checked={s.enabled}
                aria-label={`${s.enabled ? 'Pause' : 'Resume'} ${s._name} schedule`}
                className={`w-9 h-5 rounded-full transition-colors relative inline-block focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-cyan-400 ${
                  toggling === s.id ? 'opacity-50' : ''
                } ${s.enabled ? 'bg-cyan-600' : 'bg-gray-700'}`}
                title={s.enabled ? 'Pause' : 'Resume'}
              >
                <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-transform ${
                  s.enabled ? 'left-[18px]' : 'left-0.5'
                }`} />
              </button>
            ),
          },
          {
            key: 'actions',
            label: '',
            render: (s) =>
              isOperator ? (
                <div className="flex items-center gap-2">
                  <Button
                    variant="ghost"
                    size="xs"
                    onClick={() => handleTrigger(s.id, s._name)}
                    className="!px-1.5 text-cyan-400 hover:text-cyan-300"
                    title="Run now"
                    aria-label={`Run ${s._name} now`}
                    disabled={triggerSchedule.isPending}
                  >
                    &#9654;
                  </Button>
                  <Button
                    variant="ghost"
                    size="xs"
                    onClick={() => setConfirmDelete(s.id)}
                    className="!px-1.5 text-faint hover:text-red-400"
                    title="Delete schedule"
                    aria-label={`Delete ${s._name} schedule`}
                  >
                    \u2715
                  </Button>
                </div>
              ) : null,
          },
        ]}
        rows={computedSchedules}
        rowKey={(s) => s.id}
        empty={
          <>
            <p className="text-gray-400 text-sm">{schedFilterCount > 0 ? 'No schedules match the current filters' : 'No scheduled tests yet'}</p>
            {schedFilterCount === 0 && (
              <>
                <p className="text-faint text-xs mt-1">
                  Create one from the Full Stack benchmark wizard ("Add schedule" on the Review step).
                </p>
                <Link
                  to={`/projects/${projectId}/benchmarks/full-stack/new`}
                  className={buttonClassName({ variant: 'primary', size: 'xs', className: 'mt-3' })}
                >
                  New Full Stack Benchmark
                </Link>
              </>
            )}
          </>
        }
      />

      <StatusFooter
        paused={paused}
        onPauseToggle={() => setPaused(p => !p)}
        onRefresh={() => { void schedulesQuery.refetch(); }}
        lastUpdatedAt={schedulesQuery.dataUpdatedAt || null}
        intervalMs={10000}
        pills={
          <span className="text-faint">
            {computedSchedules.length} schedule{computedSchedules.length !== 1 ? 's' : ''}
            {enabledCount > 0 && ` · ${enabledCount} active`}
            {schedFilterCount > 0 && ` · ${schedFilterCount} filter${schedFilterCount !== 1 ? 's' : ''}`}
          </span>
        }
      />

      <ConfirmDialog
        open={!!scheduleToDelete}
        title="Delete schedule?"
        description={scheduleToDelete
          ? `“${scheduleToDelete.config_name || 'Unnamed'}” will stop running automatically. Existing runs are preserved.`
          : ''}
        confirmLabel="Delete schedule"
        danger
        loading={deleteSchedule.isPending}
        onConfirm={() => { if (scheduleToDelete) void handleDelete(scheduleToDelete.id); }}
        onClose={() => setConfirmDelete(null)}
      />
    </PageShell>
  );
}
