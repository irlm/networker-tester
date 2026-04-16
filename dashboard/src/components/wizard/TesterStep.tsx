import { useCallback, useEffect, useMemo, useState } from 'react';
import { testersApi, type TesterRow } from '../../api/testers';
import { CreateTesterModal } from '../CreateTesterModal';
import { useTesterSubscription } from '../../hooks/useTesterSubscription';

export type TesterOs = 'server' | 'desktop-linux' | 'desktop-windows';

export interface TesterStepProps {
  projectId: string;
  /** Cloud token from the upstream testbed step (e.g. "Azure", "AWS"). */
  cloud: string;
  /** Region token from the upstream testbed step (e.g. "eastus"). */
  region: string;
  value: string | null;
  onChange: (testerId: string | null) => void;
  /**
   * Optional OS/variant filter from the upstream testbed step. When
   * provided, only testers matching the pick are shown and the
   * "Create tester" modal is pre-filled with the corresponding OS
   * + variant. Old callers (e.g. legacy full-stack wizard) omit this
   * and get the unfiltered behaviour.
   */
  testerOs?: TesterOs;
}

/**
 * Return whether a tester row matches the wizard's `testerOs` pick.
 *
 * - `server`          → `requested_variant === 'server'` (any OS)
 * - `desktop-linux`   → Linux OS (ubuntu/debian) AND variant `desktop`
 * - `desktop-windows` → Windows OS AND variant `desktop`
 *
 * When the row lacks `requested_os` / `requested_variant` (older rows),
 * we fall back to matching on the `server` bucket so those testers don't
 * vanish from the picker.
 */
function matchesTesterOs(
  row: TesterRow,
  testerOs: TesterOs | undefined,
): boolean {
  if (!testerOs) return true;
  const os = (row.requested_os ?? '').toLowerCase();
  const variant = (row.requested_variant ?? '').toLowerCase();
  switch (testerOs) {
    case 'server':
      return variant === 'server' || (variant === '' && os === '');
    case 'desktop-linux':
      return (
        variant === 'desktop' &&
        (os.startsWith('ubuntu') || os.startsWith('debian'))
      );
    case 'desktop-windows':
      return variant === 'desktop' && os.startsWith('windows');
    default:
      return true;
  }
}

/** Map wizard OS pick → CreateTesterModal default OS + variant. */
function modalDefaultsFor(
  testerOs: TesterOs | undefined,
): { os: string; variant: string } | null {
  switch (testerOs) {
    case 'desktop-windows':
      return { os: 'windows-11', variant: 'desktop' };
    case 'desktop-linux':
      return { os: 'ubuntu-24.04', variant: 'desktop' };
    case 'server':
      return { os: 'ubuntu-24.04', variant: 'server' };
    default:
      return null;
  }
}

function normCloud(s: string): string {
  return s.trim().toLowerCase();
}

function normRegion(s: string): string {
  return s.trim().toLowerCase();
}

function formatEta(totalSeconds: number | null): string {
  if (totalSeconds == null || totalSeconds <= 0) return '—';
  const mins = Math.round(totalSeconds / 60);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem ? `${hours}h ${rem}m` : `${hours}h`;
}

type StatusVariant = 'idle' | 'busy' | 'stopped' | 'error' | 'other';

function statusVariant(row: TesterRow): StatusVariant {
  if (row.power_state === 'error') return 'error';
  if (row.power_state === 'stopped' || row.power_state === 'stopping') return 'stopped';
  if (row.allocation === 'locked' || row.allocation === 'upgrading') return 'busy';
  if (row.power_state === 'running' && row.allocation === 'idle') return 'idle';
  return 'other';
}

function statusLabel(row: TesterRow): string {
  const v = statusVariant(row);
  if (v === 'idle') return 'idle';
  if (v === 'busy') return row.allocation === 'upgrading' ? 'upgrading' : 'busy';
  if (v === 'stopped') return row.power_state;
  if (v === 'error') return 'error';
  return row.power_state;
}

function statusClass(v: StatusVariant): string {
  switch (v) {
    case 'idle':
      return 'text-green-400 border-green-500/40 bg-green-500/5';
    case 'busy':
      return 'text-yellow-300 border-yellow-500/40 bg-yellow-500/5';
    case 'stopped':
      return 'text-gray-400 border-gray-700 bg-gray-800/40';
    case 'error':
      return 'text-red-400 border-red-500/40 bg-red-500/5';
    default:
      return 'text-gray-400 border-gray-700';
  }
}

export function TesterStep({ projectId, cloud, region, value, onChange, testerOs }: TesterStepProps) {
  const [rows, setRows] = useState<TesterRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const [pendingTesterId, setPendingTesterId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!projectId) return;
    setLoading(true);
    setError(null);
    try {
      const list = await testersApi.listTesters(projectId);
      setRows(list);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load testers');
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const filtered = useMemo(
    () =>
      rows.filter(
        (r) =>
          normCloud(r.cloud) === normCloud(cloud) &&
          normRegion(r.region) === normRegion(region) &&
          matchesTesterOs(r, testerOs),
      ),
    [rows, cloud, region, testerOs],
  );

  // When filter changes or selection becomes invalid, clear.
  useEffect(() => {
    if (value && !filtered.some((r) => r.tester_id === value)) {
      onChange(null);
    }
  }, [filtered, value, onChange]);

  // Subscribe to queue updates for the visible testers so counts stay live.
  const testerIds = useMemo(() => filtered.map((r) => r.tester_id), [filtered]);
  const queueState = useTesterSubscription(projectId, testerIds);

  // Track the tester being provisioned via the modal.
  const pendingIds = useMemo(
    () => (pendingTesterId ? [pendingTesterId] : []),
    [pendingTesterId],
  );
  // We do not currently use the queue state of the pending tester — the
  // provisioning status itself comes via REST refresh. Keeping the hook
  // invocation stable avoids an extra conditional branch.
  useTesterSubscription(projectId, pendingIds);

  // If the tester that finished provisioning is now idle+running, auto-select.
  useEffect(() => {
    if (!pendingTesterId) return;
    const row = rows.find((r) => r.tester_id === pendingTesterId);
    if (!row) return;
    if (row.power_state === 'running' && row.allocation === 'idle') {
      onChange(pendingTesterId);
      setPendingTesterId(null);
    }
  }, [pendingTesterId, rows, onChange]);

  // ── Render ──
  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-semibold text-gray-200">
          Select a Runner
          <span className="text-gray-500 font-normal ml-2 font-mono text-xs">
            {cloud} / {region}
          </span>
        </h3>
        {filtered.length > 0 && (
          <button
            type="button"
            onClick={() => setShowCreate(true)}
            className="px-3 py-1.5 rounded border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
          >
            + Create another runner in {region}
          </button>
        )}
      </div>

      {loading && (
        <p className="text-xs text-gray-500" role="status">
          Loading runners…
        </p>
      )}

      {error && (
        <div
          className="mb-3 border border-red-500/30 bg-red-500/5 rounded p-3 text-red-400 text-xs"
          role="alert"
        >
          {error}
        </div>
      )}

      {/* ── State B: empty ── */}
      {!loading && !error && filtered.length === 0 && !pendingTesterId && (
        <div className="border border-dashed border-gray-800 p-4">
          <p className="text-sm text-gray-300 mb-1">
            No runners in <span className="font-mono">{region}</span> yet.
          </p>
          <p className="text-xs text-gray-500 mb-3">
            Runners are long-lived VMs that execute benchmarks against the
            testbed. Create one to continue — it takes about 2-4 minutes to
            provision and then stays available for future runs.
          </p>
          <button
            type="button"
            onClick={() => setShowCreate(true)}
            className="px-4 py-1.5 bg-cyan-600 hover:bg-cyan-500 text-white text-xs rounded transition-colors"
          >
            Create {region} runner
          </button>
        </div>
      )}

      {/* ── State C: creating ── */}
      {pendingTesterId && (
        <div
          className="border border-cyan-500/30 bg-cyan-500/5 rounded p-4 mb-3"
          data-testid="tester-step-creating"
        >
          <div className="flex items-center gap-3">
            <span className="inline-block w-3 h-3 border-2 border-cyan-400/40 border-t-cyan-400 rounded-full animate-spin" />
            <p className="text-sm text-cyan-200">
              Provisioning new runner in{' '}
              <span className="font-mono">{region}</span>…
            </p>
          </div>
          <p className="text-[11px] text-gray-500 mt-2">
            This page will auto-select the runner once it reaches the{' '}
            <span className="font-mono">running / idle</span> state. Usually 2-4
            minutes.
          </p>
        </div>
      )}

      {/* ── State A: list ── */}
      {!loading && filtered.length > 0 && (
        <div className="space-y-2" role="radiogroup" aria-label="Available runners">
          {filtered.map((row) => {
            const q = queueState[row.tester_id];
            const queueDepth = (q?.queued?.length ?? 0) + (q?.running ? 1 : 0);
            const busy = statusVariant(row) === 'busy' || queueDepth > 0;
            const variant = statusVariant(row);
            const checked = value === row.tester_id;
            return (
              <label
                key={row.tester_id}
                className={`block border p-3 cursor-pointer transition-colors ${
                  checked
                    ? 'border-cyan-500/50 bg-cyan-500/5'
                    : 'border-gray-800 hover:border-gray-600'
                }`}
              >
                <div className="flex items-center gap-3">
                  <input
                    type="radio"
                    name="tester"
                    value={row.tester_id}
                    checked={checked}
                    onChange={() => onChange(row.tester_id)}
                    className="accent-cyan-400"
                  />
                  <span className="text-sm font-medium text-gray-100 flex-1">
                    {row.name}
                  </span>
                  <span
                    className={`text-[10px] font-mono px-1.5 py-0.5 border rounded ${statusClass(variant)}`}
                  >
                    {statusLabel(row)}
                  </span>
                  {row.installer_version && (
                    <span className="text-[10px] font-mono text-gray-500">
                      v{row.installer_version}
                    </span>
                  )}
                  <span className="text-[10px] font-mono text-gray-500">
                    queue: {queueDepth}
                  </span>
                </div>

                {checked && busy && (
                  <div className="mt-2 ml-6 border border-yellow-500/30 bg-yellow-500/5 rounded p-2">
                    <p className="text-[11px] text-yellow-300">
                      This runner is currently busy. Your benchmark will be
                      queued at position{' '}
                      <span className="font-mono">{queueDepth + 1}</span>
                      {row.avg_benchmark_duration_seconds != null && (
                        <>
                          {' '}• estimated wait{' '}
                          <span className="font-mono">
                            {formatEta(
                              (queueDepth + (q?.running ? 0 : 1)) *
                                row.avg_benchmark_duration_seconds,
                            )}
                          </span>
                        </>
                      )}
                      .
                    </p>
                  </div>
                )}

                {checked && variant === 'stopped' && (
                  <div className="mt-2 ml-6 text-[11px] text-gray-400">
                    Runner is stopped -- will auto-start when the benchmark
                    launches.
                  </div>
                )}

                {checked && variant === 'error' && (
                  <div className="mt-2 ml-6 border border-red-500/30 bg-red-500/5 rounded p-2 text-[11px] text-red-400">
                    Runner is in error state. Clear the fault from the Runners
                    page before launching.
                  </div>
                )}
              </label>
            );
          })}
        </div>
      )}

      {showCreate && projectId && (
        <CreateTesterModal
          projectId={projectId}
          defaultCloud={normCloud(cloud)}
          defaultRegion={normRegion(region)}
          defaultOs={modalDefaultsFor(testerOs)?.os}
          defaultVariant={modalDefaultsFor(testerOs)?.variant}
          onCreated={(testerId) => {
            setShowCreate(false);
            setPendingTesterId(testerId);
            void refresh();
          }}
          onClose={() => {
            setShowCreate(false);
            // Refresh in case the user cancelled mid-provision; the backend
            // may still have created the row.
            void refresh();
          }}
        />
      )}
    </div>
  );
}
