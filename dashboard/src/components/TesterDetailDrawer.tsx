import { useCallback, useEffect, useMemo, useState } from 'react';
import { useAsyncEffect } from '../hooks/useAsyncEffect';
import { useNow } from '../hooks/useNow';
import {
  testersApi,
  type TesterRow,
  type CostEstimate,
} from '../api/testers';
import { useTesterSubscription } from '../hooks/useTesterSubscription';
import { useProjectStore } from '../stores/projectStore';
import { StatusBadge } from './common/StatusBadge';
import { DetailList } from './common/DetailList';
import { RotateKeyDialog } from './RotateKeyDialog';

interface TesterDetailDrawerProps {
  projectId: string;
  tester: TesterRow | null;
  onClose: () => void;
  onChanged: () => void;
}

const GITHUB_RELEASES = 'https://github.com/irlm/networker-tester/releases';

const HOURS = Array.from({ length: 24 }, (_, h) => h);

/** Strip a leading `v` so `v0.28.16` and `0.28.16` compare equal. */
function normalizeVersion(v: string): string {
  return v.replace(/^v/, '');
}

function formatDate(value: string | null): string {
  if (!value) return '—';
  try {
    return new Date(value).toLocaleString();
  } catch {
    return value;
  }
}

function formatDuration(seconds: number | null): string {
  if (seconds == null) return '—';
  if (seconds < 60) return `${Math.round(seconds)}s`;
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return `${m}m ${s}s`;
}

function stateBadgeStatus(
  power: TesterRow['power_state'],
  allocation: TesterRow['allocation'],
): string {
  if (power === 'error') return 'failed';
  if (power === 'running' && allocation === 'locked') return 'running';
  if (power === 'running') return 'online';
  if (power === 'starting' || power === 'provisioning') return 'deploying';
  if (power === 'stopping') return 'pending';
  if (power === 'stopped') return 'offline';
  if (power === 'upgrading') return 'deploying';
  return 'offline';
}

type ActionState = 'idle' | 'busy';

export function TesterDetailDrawer({
  projectId,
  tester: row,
  onClose,
  onChanged,
}: TesterDetailDrawerProps) {
  // The list endpoint is a slim projection (no vm_name/public_ip/ssh_user/
  // created_by/installer_version/OS facts) — fetch the full detail record and
  // render it merged with the freshest polled row (row wins for live status,
  // detail supplies the identity fields the list never carries). Refetches
  // when the row meaningfully changes (updated_at bumps after actions).
  const [detail, setDetail] = useState<TesterRow | null>(null);
  // Primitive deps: the polled list returns a NEW row object every tick — a
  // dep on `row` itself would refetch the detail every 5s. tester_id +
  // updated_at capture "different runner or meaningfully changed".
  const rowId = row?.tester_id;
  const rowUpdatedAt = row?.updated_at;
  useEffect(() => {
    if (!rowId) return;
    let cancelled = false;
    testersApi
      .getTester(projectId, rowId)
      .then((d) => { if (!cancelled) setDetail(d); })
      .catch(() => { /* slim row remains the fallback */ });
    return () => { cancelled = true; };
  }, [projectId, rowId, rowUpdatedAt]);
  const tester: TesterRow | null = useMemo(
    () => (row
      ? { ...(detail && detail.tester_id === row.tester_id ? detail : {}), ...row }
      : null),
    [row, detail],
  );

  const [costEstimate, setCostEstimate] = useState<CostEstimate | null>(null);
  const [costError, setCostError] = useState<string | null>(null);
  const [actionState, setActionState] = useState<ActionState>('idle');
  const [actionError, setActionError] = useState<string | null>(null);
  const [confirmForceStop, setConfirmForceStop] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [confirmForceDelete, setConfirmForceDelete] = useState(false);
  // A prior delete whose cloud VM teardown failed (e.g. dead creds) leaves the
  // tester with this status — surface the force-delete escape hatch then.
  const deleteFailed = /delete failed/i.test(tester?.status_message ?? '');
  const [rotatingKey, setRotatingKey] = useState(false);
  const [latestVersion, setLatestVersion] = useState<string | null>(null);
  const [editingSchedule, setEditingSchedule] = useState(false);
  const [scheduleEnabled, setScheduleEnabled] = useState(false);
  const [scheduleHour, setScheduleHour] = useState(23);

  // RBAC: mutating controls (start/stop/delete/reinstall/force-stop/probe)
  // are operator+ only — viewers get a read-only drawer. Same derivation as
  // useProject().isOperator, read from the store directly so the drawer does
  // not depend on router params. Guarded by TesterDetailDrawer.rbac.test.tsx.
  const projectRole = useProjectStore((s) => s.activeProjectRole);
  const isOperator = projectRole === 'admin' || projectRole === 'operator';

  const testerIds = useMemo(
    () => (tester ? [tester.tester_id] : []),
    [tester],
  );
  const queueMap = useTesterSubscription(projectId, testerIds);
  const queueState = tester ? queueMap[tester.tester_id] : undefined;

  // The three resets ran synchronously in the effect body. Deferring them with
  // the fetch keeps the same visible behaviour — at most one extra frame of the
  // previous tester's cost — without the cascading render. useAsyncEffect owns
  // the cancellation flag this effect used to hand-roll.
  useAsyncEffect((cancelled) => {
    if (!tester) return;
    setCostEstimate(null);
    setCostError(null);
    setEditingSchedule(false);

    const cost = testersApi
      .getCostEstimate(projectId, tester.tester_id)
      .then((c) => {
        if (!cancelled()) setCostEstimate(c);
      })
      .catch((e) => {
        if (!cancelled())
          setCostError(e instanceof Error ? e.message : 'Cost unavailable');
      });

    // Latest released version — served from the dashboard's server-side cache.
    const version = testersApi
      .refreshLatestVersion(projectId)
      .then((r) => {
        if (!cancelled()) setLatestVersion(r.latest_version ?? null);
      })
      .catch(() => {
        // Non-fatal — leave "Latest known" as an em dash.
      });

    return Promise.all([cost, version]);
  }, [projectId, tester]);

  // Escape closes the drawer — unless a confirm dialog is open (it handles
  // its own Escape) or an action is in flight.
  useEffect(() => {
    if (!tester) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (confirmForceStop || confirmDelete || confirmForceDelete) return;
      onClose();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [tester, confirmForceStop, confirmDelete, confirmForceDelete, onClose]);

  const run = useCallback(
    async (fn: () => Promise<unknown>) => {
      setActionState('busy');
      setActionError(null);
      try {
        await fn();
        onChanged();
      } catch (e) {
        setActionError(e instanceof Error ? e.message : 'Action failed');
      } finally {
        setActionState('idle');
      }
    },
    [onChanged],
  );

  // Clock from state, not read during render (react-hooks/purity). Called
  // before the early return so hook order stays stable.
  const now = useNow();

  if (!tester) return null;

  const isError = tester.power_state === 'error';
  const isBusy = actionState === 'busy';
  // Agent api-key expiry status (V044): a non-null expiry in the past is expired.
  const keyExpiry = {
    expired: Boolean(
      tester.api_key_expires_at &&
        new Date(tester.api_key_expires_at).getTime() <= now,
    ),
  };
  const isRunningOrQueued =
    tester.allocation !== 'idle' ||
    Boolean(queueState?.running) ||
    (queueState?.queued?.length ?? 0) > 0;

  const installerVersion = tester.installer_version ?? '—';
  const updateAvailable = Boolean(
    latestVersion &&
      tester.installer_version &&
      normalizeVersion(tester.installer_version) !==
        normalizeVersion(latestVersion),
  );

  return (
    <div className="fixed inset-0 z-50 flex justify-end" data-testid="tester-detail-drawer">
      <div
        className="absolute inset-0 bg-black/40 slide-over-backdrop"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="tester-detail-title"
        className="relative w-full md:w-[560px] md:max-w-[95vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        <div className="p-4 md:p-6 space-y-6">
          <div className="flex items-center justify-between">
            <div>
              <h3 id="tester-detail-title" className="text-lg font-bold text-gray-100">
                {tester.name}
              </h3>
              <p className="text-xs text-gray-400 font-mono">
                {tester.cloud} / {tester.region} · {tester.tester_id.slice(0, 8)}
              </p>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="text-gray-400 hover:text-gray-300 text-sm"
              aria-label="Close"
            >
              &#x2715;
            </button>
          </div>

          {actionError && (
            <div
              role="alert"
              className="bg-red-500/10 border border-red-500/30 rounded p-2"
            >
              <p className="text-red-400 text-sm">{actionError}</p>
            </div>
          )}

          {/* ── Error recovery panel ────────────────────────────────────── */}
          {isError && (
            <section
              data-testid="fix-tester-panel"
              className="border border-red-500/40 bg-red-500/5 rounded p-4 space-y-3"
            >
              <div>
                <h4 className="text-sm font-bold text-red-400">Fix runner first</h4>
                <p className="text-xs text-red-300/80 mt-1">
                  This runner is in an error state. Resolve the fault before
                  queueing more work.
                </p>
                {tester.status_message && (
                  <p className="text-xs text-gray-400 mt-2 font-mono">
                    {tester.status_message}
                  </p>
                )}
              </div>
              {isOperator && (
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={isBusy}
                  onClick={() => run(() => testersApi.probe(projectId, tester.tester_id))}
                  className="px-3 py-1 text-xs rounded border border-cyan-500/50 text-cyan-400 hover:bg-cyan-500/10 disabled:opacity-50"
                >
                  Run probe
                </button>
                <button
                  type="button"
                  disabled={isBusy}
                  onClick={() =>
                    run(() =>
                      testersApi.upgradeTester(projectId, tester.tester_id, {
                        confirm: true,
                      }),
                    )
                  }
                  className="px-3 py-1 text-xs rounded border border-purple-500/50 text-purple-400 hover:bg-purple-500/10 disabled:opacity-50"
                >
                  Reinstall runner
                </button>
                <button
                  type="button"
                  disabled={isBusy}
                  onClick={() => setConfirmForceStop(true)}
                  className="px-3 py-1 text-xs rounded border border-amber-500/50 text-amber-400 hover:bg-amber-500/10 disabled:opacity-50"
                >
                  Force to stopped
                </button>
                <button
                  type="button"
                  disabled={isBusy}
                  onClick={() => setConfirmDelete(true)}
                  className="px-3 py-1 text-xs rounded border border-red-500/50 text-red-400 hover:bg-red-500/10 disabled:opacity-50"
                >
                  Delete runner
                </button>
              </div>
              )}
            </section>
          )}

          {/* ── Status ─────────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Status</h4>
            <div className="flex items-center gap-2">
              <StatusBadge
                status={stateBadgeStatus(tester.power_state, tester.allocation)}
                label={`${tester.power_state} · ${tester.allocation}`}
              />
              {tester.allocation === 'locked' && tester.locked_by_config_id && (
                <span className="text-xs text-gray-400 font-mono">
                  locked by {tester.locked_by_config_id.slice(0, 8)}
                </span>
              )}
            </div>
            {tester.status_message && !isError && (
              <p className="text-xs text-gray-400 mt-2 font-mono">
                {tester.status_message}
              </p>
            )}
          </section>

          {/* ── Identity ───────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Identity</h4>
            <DetailList
              rows={[
                { label: 'Cloud', value: tester.cloud },
                { label: 'Region', value: tester.region },
                { label: 'VM size', value: tester.vm_size },
                // Discovered OS facts win (agent-reported post-provision);
                // fall back to what was requested at creation.
                {
                  label: 'OS',
                  value: tester.os_distro
                    ? `${`${tester.os_distro} ${tester.os_version ?? ''}`.trim()}${tester.os_arch ? ` (${tester.os_arch})` : ''}`
                    : tester.requested_os
                      ? `${tester.requested_os}${tester.requested_variant ? ` · ${tester.requested_variant}` : ''} (requested)`
                      : null,
                  title: tester.os_kernel ? `kernel ${tester.os_kernel}` : undefined,
                },
                { label: 'VM name', value: tester.vm_name },
                { label: 'Public IP', value: tester.public_ip },
                { label: 'SSH user', value: tester.ssh_user },
                { label: 'Created by', value: tester.created_by },
                { label: 'Created at', value: formatDate(tester.created_at) },
              ]}
            />
          </section>

          {/* ── Version ────────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Version</h4>
            <div className="text-xs space-y-1">
              <div className="flex items-center gap-2">
                <span className="text-gray-400">Installed:</span>
                <span className="text-gray-300 font-mono">{installerVersion}</span>
                {tester.last_installed_at && (
                  <span className="text-gray-400">
                    · {formatDate(tester.last_installed_at)}
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2">
                <span className="text-gray-400">Latest known:</span>
                <span className="text-gray-300 font-mono">{latestVersion ?? '—'}</span>
                {updateAvailable && (
                  <span className="px-1.5 py-0.5 text-[10px] rounded bg-purple-500/20 text-purple-300 border border-purple-500/30">
                    Update available
                  </span>
                )}
              </div>
              <a
                href={GITHUB_RELEASES}
                target="_blank"
                rel="noreferrer"
                className="text-cyan-400 hover:text-cyan-300"
              >
                View changelog →
              </a>
            </div>
          </section>

          {/* ── Cost ───────────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">
              Cost estimate
            </h4>
            {costError && (
              <p className="text-xs text-red-400">{costError}</p>
            )}
            {!costError && !costEstimate && (
              <p className="text-xs text-gray-400">Loading…</p>
            )}
            {costEstimate && (
              <DetailList
                rows={[
                  { label: 'Hourly', value: `$${costEstimate.hourly_usd.toFixed(3)}` },
                  { label: 'Monthly (always-on)', value: `$${costEstimate.monthly_always_on_usd.toFixed(2)}` },
                  { label: 'Monthly (with schedule)', value: `$${costEstimate.monthly_with_schedule_usd.toFixed(2)}`, accent: true },
                ]}
              />
            )}
          </section>

          {/* ── Usage ──────────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Usage</h4>
            <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
              <dt className="text-gray-400">Benchmarks run</dt>
              <dd className="text-gray-300 font-mono">{tester.benchmark_run_count}</dd>
              <dt className="text-gray-400">Avg duration</dt>
              <dd className="text-gray-300 font-mono">
                {formatDuration(tester.avg_benchmark_duration_seconds)}
              </dd>
              <dt className="text-gray-400">Last used</dt>
              <dd className="text-gray-300 font-mono">{formatDate(tester.last_used_at)}</dd>
            </dl>
          </section>

          {/* ── Auto-shutdown ──────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">
              Auto-shutdown
            </h4>
            <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs mb-3">
              <dt className="text-gray-400">Enabled</dt>
              <dd className="text-gray-300 font-mono">
                {tester.auto_shutdown_enabled ? 'yes' : 'no'}
              </dd>
              <dt className="text-gray-400">Local hour</dt>
              <dd className="text-gray-300 font-mono">
                {String(tester.auto_shutdown_local_hour).padStart(2, '0')}:00
              </dd>
              <dt className="text-gray-400">Next shutdown</dt>
              <dd className="text-gray-300 font-mono">
                {formatDate(tester.next_shutdown_at)}
              </dd>
              {tester.shutdown_deferral_count > 0 && (
                <>
                  <dt className="text-gray-400">Deferrals</dt>
                  <dd className="text-amber-400 font-mono">
                    {tester.shutdown_deferral_count}
                  </dd>
                </>
              )}
            </dl>
            {editingSchedule && (
              <div className="border border-gray-800 rounded p-3 mb-3 flex flex-wrap items-center gap-3">
                <label className="flex items-center gap-2 text-xs text-gray-300 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={scheduleEnabled}
                    onChange={(e) => setScheduleEnabled(e.target.checked)}
                    className="accent-cyan-500"
                  />
                  Auto-shutdown
                </label>
                <label className="flex items-center gap-2 text-xs text-gray-400">
                  at
                  <select
                    value={scheduleHour}
                    disabled={!scheduleEnabled}
                    onChange={(e) => setScheduleHour(Number(e.target.value))}
                    className="bg-[var(--bg-base)] border border-gray-700 rounded px-2 py-1 text-xs text-gray-200 font-mono focus:outline-none focus:border-cyan-500 disabled:opacity-50"
                    aria-label="Local shutdown hour"
                  >
                    {HOURS.map((h) => (
                      <option key={h} value={h}>
                        {String(h).padStart(2, '0')}:00
                      </option>
                    ))}
                  </select>
                </label>
                <div className="flex gap-2 ml-auto">
                  <button
                    type="button"
                    disabled={isBusy}
                    onClick={() => setEditingSchedule(false)}
                    className="px-3 py-1 text-xs text-gray-400 hover:text-gray-200 disabled:opacity-50"
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    disabled={isBusy}
                    onClick={() => {
                      setEditingSchedule(false);
                      run(() =>
                        testersApi.updateSchedule(projectId, tester.tester_id, {
                          auto_shutdown_enabled: scheduleEnabled,
                          auto_shutdown_local_hour: scheduleHour,
                        }),
                      );
                    }}
                    className="px-3 py-1 text-xs rounded bg-cyan-600 hover:bg-cyan-500 text-white disabled:opacity-50"
                  >
                    Save
                  </button>
                </div>
              </div>
            )}
            {isOperator && (
            <div className="flex flex-wrap gap-2">
              {!editingSchedule && (
                <button
                  type="button"
                  disabled={isBusy}
                  className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 disabled:opacity-50"
                  onClick={() => {
                    setScheduleEnabled(tester.auto_shutdown_enabled);
                    setScheduleHour(tester.auto_shutdown_local_hour);
                    setEditingSchedule(true);
                  }}
                >
                  Edit schedule
                </button>
              )}
              <button
                type="button"
                disabled={isBusy}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 disabled:opacity-50"
                onClick={() =>
                  run(() =>
                    testersApi.postpone(projectId, tester.tester_id, {
                      add_hours: 2,
                    }),
                  )
                }
              >
                Postpone 2h
              </button>
              <button
                type="button"
                disabled={isBusy || !tester.auto_shutdown_enabled}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-red-500 hover:text-red-400 disabled:opacity-50"
                onClick={() =>
                  run(() =>
                    testersApi.updateSchedule(projectId, tester.tester_id, {
                      auto_shutdown_enabled: false,
                    }),
                  )
                }
              >
                Disable
              </button>
            </div>
            )}
          </section>

          {/* ── Recovery ───────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Recovery</h4>
            <div className="flex items-center gap-2 mb-2">
              <span className="text-xs text-gray-400">Auto-probe:</span>
              <span className="text-xs text-gray-300 font-mono">
                {tester.auto_probe_enabled ? 'enabled' : 'disabled'}
              </span>
            </div>
            {isOperator && (
              <button
                type="button"
                disabled={isBusy}
                onClick={() => run(() => testersApi.probe(projectId, tester.tester_id))}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400 disabled:opacity-50"
              >
                Run probe now
              </button>
            )}
          </section>

          {/* ── Agent key ──────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">
              Agent key
            </h4>
            <div className="flex items-center gap-2 mb-2">
              <span className="text-xs text-gray-400">Last seen:</span>
              <span className="text-xs text-gray-300 font-mono">
                {tester.api_key_last_used_at
                  ? formatDate(tester.api_key_last_used_at)
                  : 'never'}
              </span>
            </div>
            <div className="flex items-center gap-2 mb-2">
              <span className="text-xs text-gray-400">Key expiry:</span>
              {keyExpiry.expired ? (
                <StatusBadge status="failed" label="expired" />
              ) : tester.api_key_expires_at ? (
                <StatusBadge
                  status="waiting"
                  label={`expires ${formatDate(tester.api_key_expires_at)}`}
                />
              ) : (
                <span className="text-xs text-gray-300 font-mono">no expiry</span>
              )}
            </div>
            {isOperator && (
              <button
                type="button"
                onClick={() => setRotatingKey(true)}
                className="px-3 py-1 text-xs rounded border border-gray-700 text-gray-400 hover:border-cyan-500 hover:text-cyan-400"
              >
                Rotate key
              </button>
            )}
          </section>

          {/* ── Queue ──────────────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">Queue</h4>
            {queueState?.running ? (
              <div className="border border-cyan-500/30 bg-cyan-500/5 rounded p-2 mb-2 text-xs">
                <div className="text-cyan-400 font-mono">
                  running: {queueState.running.name}
                </div>
              </div>
            ) : (
              <p className="text-xs text-gray-400">No running benchmark.</p>
            )}
            {queueState && queueState.queued.length > 0 ? (
              <ol className="space-y-1 text-xs font-mono">
                {queueState.queued.map((q) => (
                  <li key={q.config_id} className="text-gray-400">
                    #{q.position ?? '?'} {q.name}
                  </li>
                ))}
              </ol>
            ) : (
              <p className="text-xs text-gray-400">No queued benchmarks.</p>
            )}
          </section>

          {/* ── Recent activity ────────────────────────────────────────── */}
          <section>
            <h4 className="text-xs uppercase tracking-wide text-gray-400 mb-2">
              Recent activity
            </h4>
            {/* Placeholder: dashboard has no service_log table (Task 11 note). */}
            <p className="text-xs text-gray-400">Audit log coming soon.</p>
          </section>

          {/* ── Danger zone (operator+ only — viewers get read-only) ──── */}
          {isOperator && (
          <section className="border-t border-gray-800 pt-4">
            <h4 className="text-xs uppercase tracking-wide text-red-400 mb-2">
              Danger zone
            </h4>
            <div className="flex flex-wrap gap-2">
              {tester.power_state === 'stopped' ? (
                <button
                  type="button"
                  disabled={isBusy}
                  onClick={() => run(() => testersApi.startTester(projectId, tester.tester_id))}
                  className="px-3 py-1 text-xs rounded border border-emerald-500/50 text-emerald-400 hover:bg-emerald-500/10 disabled:opacity-50"
                >
                  Start runner
                </button>
              ) : (
                <button
                  type="button"
                  disabled={
                    isBusy ||
                    isRunningOrQueued ||
                    tester.power_state !== 'running'
                  }
                  onClick={() => run(() => testersApi.stopTester(projectId, tester.tester_id))}
                  className="px-3 py-1 text-xs rounded border border-amber-500/50 text-amber-400 hover:bg-amber-500/10 disabled:opacity-50"
                  title={
                    tester.power_state !== 'running'
                      ? `Cannot stop in power_state=${tester.power_state}`
                      : undefined
                  }
                >
                  Stop runner
                </button>
              )}
              <button
                type="button"
                disabled={isBusy || isRunningOrQueued}
                onClick={() => setConfirmDelete(true)}
                className="px-3 py-1 text-xs rounded border border-red-500/50 text-red-400 hover:bg-red-500/10 disabled:opacity-50"
              >
                Delete runner
              </button>
              {deleteFailed && (
                <button
                  type="button"
                  disabled={isBusy || isRunningOrQueued}
                  onClick={() => setConfirmForceDelete(true)}
                  title="The cloud VM delete failed (e.g. credentials are gone). Remove the record anyway — the VM may be orphaned."
                  className="px-3 py-1 text-xs rounded border border-red-500 bg-red-500/10 text-red-300 hover:bg-red-500/20 disabled:opacity-50"
                >
                  Force delete
                </button>
              )}
            </div>
            {isRunningOrQueued && (
              <p className="text-xs text-gray-400 mt-2">
                Disabled while benchmarks are running or queued.
              </p>
            )}
          </section>
          )}
        </div>
      </div>

      {/* ── Confirm force-stop ─────────────────────────────────────────── */}
      {confirmForceStop && (
        <ConfirmDialog
          title="Force runner to stopped"
          message="This marks the runner stopped without waiting for a clean shutdown. Queued benchmarks remain locked until manually released. Continue?"
          confirmLabel="Force stop"
          danger
          onConfirm={() => {
            setConfirmForceStop(false);
            run(() =>
              testersApi.forceStop(projectId, tester.tester_id, {
                confirm: true,
                reason: 'manual force-stop from UI',
              }),
            );
          }}
          onCancel={() => setConfirmForceStop(false)}
        />
      )}

      {/* ── Confirm delete ─────────────────────────────────────────────── */}
      {confirmDelete && (
        <ConfirmDialog
          title={`Delete runner "${tester.name}"?`}
          message="The VM will be deprovisioned. This cannot be undone."
          confirmLabel="Delete"
          danger
          onConfirm={() => {
            setConfirmDelete(false);
            run(async () => {
              // Delete is ASYNC (202 → the VM deprovisions in the background).
              // Do NOT close the drawer here: refresh the list and stay open so
              // the outcome is visible — the row goes deleting → gone (the parent
              // then closes the drawer) or → "delete failed" (the Force-delete
              // button below appears). Closing eagerly hid a failed background
              // delete entirely (P2-15).
              await testersApi.deleteTester(projectId, tester.tester_id);
              onChanged();
            });
          }}
          onCancel={() => setConfirmDelete(false)}
        />
      )}

      {/* ── Confirm FORCE delete ───────────────────────────────────────────── */}
      {confirmForceDelete && (
        <ConfirmDialog
          title={`Force-delete runner "${tester.name}"?`}
          message={
            'The cloud VM could not be deleted (its credentials are likely gone). '
            + 'Force-delete removes the runner record anyway — the VM may still exist '
            + 'and must be deleted in your cloud console. This cannot be undone.'
          }
          confirmLabel="Force delete"
          danger
          onConfirm={() => {
            setConfirmForceDelete(false);
            run(async () => {
              await testersApi.deleteTester(projectId, tester.tester_id, true);
              onChanged();
              onClose();
            });
          }}
          onCancel={() => setConfirmForceDelete(false)}
        />
      )}

      {rotatingKey && (
        <RotateKeyDialog
          projectId={projectId}
          testerId={tester.tester_id}
          testerName={tester.name}
          onClose={() => setRotatingKey(false)}
          onRotated={() => onChanged()}
        />
      )}
    </div>
  );
}

interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

function ConfirmDialog({
  title,
  message,
  confirmLabel,
  danger,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onCancel();
      }
    };
    document.addEventListener('keydown', handler, true);
    return () => document.removeEventListener('keydown', handler, true);
  }, [onCancel]);

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60" onClick={onCancel} aria-hidden="true" />
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="confirm-dialog-title"
        className="relative bg-[var(--bg-base)] border border-gray-800 rounded p-5 w-[360px] max-w-[90vw]"
      >
        <h4 id="confirm-dialog-title" className="text-sm font-bold text-gray-100 mb-2">{title}</h4>
        <p className="text-xs text-gray-400 mb-4">{message}</p>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="px-3 py-1 text-xs text-gray-400 hover:text-gray-200"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className={`px-3 py-1 text-xs rounded ${
              danger
                ? 'bg-red-600 hover:bg-red-500 text-white'
                : 'bg-cyan-600 hover:bg-cyan-500 text-white'
            }`}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
