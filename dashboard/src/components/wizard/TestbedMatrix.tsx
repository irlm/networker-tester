import { useState, useEffect, useMemo } from 'react';
import { testersApi, type TesterRow } from '../../api/testers';
import { TestbedRow } from './TestbedRow';
import type { TestbedState } from './testbed-constants';
import { makeTestbed, updateTestbedState } from './testbed-constants';

// ── Runner helpers ────────────────────────────────────────────────────

function testerStatusClass(row: TesterRow): string {
  if (row.power_state === 'error') return 'text-red-400 border-red-500/40 bg-red-500/5';
  if (row.power_state === 'stopped' || row.power_state === 'stopping') return 'text-gray-400 border-gray-700 bg-gray-800/40';
  if (row.allocation === 'locked' || row.allocation === 'upgrading') return 'text-yellow-300 border-yellow-500/40 bg-yellow-500/5';
  if (row.power_state === 'running' && row.allocation === 'idle') return 'text-green-400 border-green-500/40 bg-green-500/5';
  return 'text-gray-400 border-gray-700';
}

function testerStatusLabel(row: TesterRow): string {
  if (row.power_state === 'error') return 'error';
  if (row.allocation === 'locked') return 'busy';
  if (row.allocation === 'upgrading') return 'upgrading';
  if (row.power_state === 'running' && row.allocation === 'idle') return 'idle';
  return row.power_state;
}

// ── Props ─────────────────────────────────────────────────────────────

export interface TestbedMatrixProps {
  projectId: string;
  testbeds: TestbedState[];
  onTestbedsChange: (testbeds: TestbedState[]) => void;
  runnerMode: 'auto' | 'specific';
  onRunnerModeChange: (mode: 'auto' | 'specific') => void;
  selectedTesterId: string | null;
  onTesterIdChange: (id: string | null) => void;
  proxyWarning: boolean;
}

// ── Component ─────────────────────────────────────────────────────────

export function TestbedMatrix({
  projectId,
  testbeds,
  onTestbedsChange,
  runnerMode,
  onRunnerModeChange,
  selectedTesterId,
  onTesterIdChange,
  proxyWarning,
}: TestbedMatrixProps) {
  const [testbedKey, setTestbedKey] = useState(() => testbeds.length);
  const [testers, setTesters] = useState<TesterRow[]>([]);
  const [testersLoading, setTestersLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    testersApi.listTesters(projectId)
      .then(rows => { if (!cancelled) setTesters(rows); })
      .catch(() => { if (!cancelled) setTesters([]); })
      .finally(() => { if (!cancelled) setTestersLoading(false); });
    return () => { cancelled = true; };
  }, [projectId]);

  const runnerStats = useMemo(() => {
    const online = testers.filter(t => t.power_state === 'running');
    const busy = online.filter(t => t.allocation === 'locked');
    const idle = online.filter(t => t.allocation === 'idle');
    return { online: online.length, busy: busy.length, idle: idle.length };
  }, [testers]);

  const addTestbed = (cloud?: string, os?: 'linux' | 'windows') => {
    const k = testbedKey;
    setTestbedKey(k + 1);
    onTestbedsChange([...testbeds, makeTestbed(k, cloud, os)]);
  };

  const removeTestbed = (key: number) => {
    onTestbedsChange(testbeds.filter(c => c.key !== key));
  };

  const updateTestbed = (key: number, patch: Partial<TestbedState>) => {
    onTestbedsChange(updateTestbedState(testbeds, key, patch));
  };

  return (
    <div>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-semibold text-gray-200">Configure Testbeds</h3>
        <button
          onClick={() => addTestbed()}
          className="px-3 py-1.5 border border-gray-700 text-xs text-gray-200 hover:border-cyan-500 transition-colors"
        >
          + Add Testbed
        </button>
      </div>

      {testbeds.length === 0 && (
        <div className="border border-dashed border-gray-800 p-4">
          <p className="text-xs text-gray-500 mb-3">No testbeds configured. Add one to define where tests run.</p>
          <div className="flex flex-wrap gap-2">
            {[
              { cloud: 'Azure', os: 'linux' as const },
              { cloud: 'Azure', os: 'windows' as const },
              { cloud: 'AWS', os: 'linux' as const },
              { cloud: 'GCP', os: 'linux' as const },
            ].map(({ cloud, os }) => (
              <button
                key={`${cloud}-${os}`}
                onClick={() => addTestbed(cloud, os)}
                className="px-3 py-1.5 text-xs font-mono border border-gray-700 text-gray-300 hover:border-cyan-500 hover:text-cyan-300 transition-colors"
              >
                + {cloud} / {os === 'linux' ? 'Linux' : 'Windows'}
              </button>
            ))}
          </div>
        </div>
      )}

      {proxyWarning && (
        <div className="mb-3 border border-yellow-500/30 bg-yellow-500/5 p-3">
          <p className="text-xs text-yellow-300">
            Every testbed must have at least one reverse proxy selected before proceeding.
          </p>
        </div>
      )}

      <div className="space-y-2">
        {testbeds.map((testbed, idx) => (
          <TestbedRow
            key={testbed.key}
            testbed={testbed}
            index={idx}
            onUpdate={updateTestbed}
            onRemove={removeTestbed}
          />
        ))}
      </div>

      {/* Runner selection */}
      {testbeds.length > 0 && (
        <div className="mt-6 border border-gray-800 p-4">
          <div className="flex items-center justify-between mb-3">
            <h4 className="text-xs font-semibold text-gray-300 uppercase tracking-wider">Runner Assignment</h4>
            {!testersLoading && (
              <span className="text-[10px] font-mono text-gray-600">
                {runnerStats.idle} idle / {runnerStats.online} online
              </span>
            )}
          </div>

          <div className="flex gap-2 mb-3">
            {(['auto', 'specific'] as const).map(mode => (
              <button
                key={mode}
                onClick={() => { onRunnerModeChange(mode); if (mode === 'auto') onTesterIdChange(null); }}
                className={`px-2.5 py-1 text-xs border transition-colors ${
                  runnerMode === mode
                    ? 'bg-cyan-500/10 border-cyan-500/40 text-cyan-300'
                    : 'border-gray-700 text-gray-500 hover:text-gray-300'
                }`}
              >
                {mode === 'auto' ? 'Auto-pick' : 'Pick specific'}
              </button>
            ))}
          </div>

          {runnerMode === 'auto' && (
            <p className="text-xs text-gray-500">First available idle runner will be assigned.</p>
          )}

          {runnerMode === 'specific' && (
            <div className="space-y-1">
              {testersLoading && <p className="text-xs text-gray-500 motion-safe:animate-pulse">Loading runners...</p>}
              {!testersLoading && testers.length === 0 && <p className="text-xs text-gray-500">No runners available.</p>}
              {!testersLoading && testers.length > 0 && testers.map(row => {
                const isOnline = row.power_state === 'running';
                const isIdle = isOnline && row.allocation === 'idle';
                const isBusy = isOnline && row.allocation === 'locked';
                const checked = selectedTesterId === row.tester_id;
                return (
                  <label
                    key={row.tester_id}
                    className={`block border p-2.5 transition-colors ${
                      !isOnline ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'
                    } ${
                      checked ? 'border-cyan-500/50 bg-cyan-500/5' : 'border-gray-800 hover:border-gray-600'
                    }`}
                  >
                    <div className="flex items-center gap-3">
                      <input
                        type="radio"
                        name="runner-specific"
                        value={row.tester_id}
                        checked={checked}
                        disabled={!isOnline}
                        onChange={() => onTesterIdChange(row.tester_id)}
                        className="accent-cyan-400"
                      />
                      <span className={`w-1.5 h-1.5 rounded-full ${isIdle ? 'bg-green-400' : isBusy ? 'bg-yellow-400' : 'bg-gray-600'}`} />
                      <span className="text-sm font-medium text-gray-100 flex-1">{row.name}</span>
                      <span className="text-[10px] font-mono text-gray-500">{row.cloud} / {row.region}</span>
                      <span className={`text-[10px] font-mono px-1.5 py-0.5 border rounded ${testerStatusClass(row)}`}>
                        {testerStatusLabel(row)}
                      </span>
                    </div>
                  </label>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
