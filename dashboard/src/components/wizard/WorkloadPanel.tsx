import { useCallback } from 'react';
import type { ModeGroup } from '../../api/types';
import { ModeSelector } from '../common/ModeSelector';
import { PayloadSelector } from '../common/PayloadSelector';
import { THROUGHPUT_IDS } from '../../lib/chart';

export interface WorkloadPanelProps {
  modeGroups: ModeGroup[];
  selectedModes: Set<string>;
  onModesChange: (modes: Set<string>) => void;
  runs: number;
  onRunsChange: (n: number) => void;
  concurrency: number;
  onConcurrencyChange: (n: number) => void;
  timeoutMs: number;
  onTimeoutChange: (n: number) => void;
  selectedPayloads: Set<string>;
  onPayloadsChange: (payloads: Set<string>) => void;
  /** Show advanced options (insecure, connection reuse, capture mode). Default true. */
  showAdvanced?: boolean;
  insecure?: boolean;
  onInsecureChange?: (v: boolean) => void;
  connectionReuse?: boolean;
  onConnectionReuseChange?: (v: boolean) => void;
  captureMode?: 'none' | 'tester' | 'endpoint' | 'both';
  onCaptureModeChange?: (v: 'none' | 'tester' | 'endpoint' | 'both') => void;
}

export function WorkloadPanel({
  modeGroups,
  selectedModes,
  onModesChange,
  runs,
  onRunsChange,
  concurrency,
  onConcurrencyChange,
  timeoutMs,
  onTimeoutChange,
  selectedPayloads,
  onPayloadsChange,
  showAdvanced = true,
  insecure = false,
  onInsecureChange,
  connectionReuse = true,
  onConnectionReuseChange,
  captureMode = 'none',
  onCaptureModeChange,
}: WorkloadPanelProps) {
  const handleModeToggle = useCallback((id: string) => {
    const next = new Set(selectedModes);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onModesChange(next);
  }, [selectedModes, onModesChange]);

  const handleGroupToggle = useCallback((ids: string[], allSelected: boolean) => {
    const next = new Set(selectedModes);
    for (const id of ids) {
      if (allSelected) next.delete(id);
      else next.add(id);
    }
    onModesChange(next);
  }, [selectedModes, onModesChange]);

  const handlePayloadToggle = useCallback((value: string) => {
    const next = new Set(selectedPayloads);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    onPayloadsChange(next);
  }, [selectedPayloads, onPayloadsChange]);

  const hasThroughput = [...selectedModes].some(m => (THROUGHPUT_IDS as readonly string[]).includes(m));

  return (
    <div className="space-y-4">
      <h3 className="text-sm font-semibold text-gray-200 mb-2">Workload Configuration</h3>

      {modeGroups.length > 0 && (
        <div>
          <label className="text-xs text-gray-500 mb-2 block">Modes</label>
          <ModeSelector
            modeGroups={modeGroups}
            selectedModes={selectedModes}
            onToggle={handleModeToggle}
            onToggleGroup={handleGroupToggle}
          />
        </div>
      )}

      <div className="grid grid-cols-3 gap-4">
        <div>
          <label htmlFor="wl-runs" className="text-xs text-gray-500 mb-1 block">Runs</label>
          <input
            id="wl-runs"
            type="number"
            min={1}
            value={runs}
            onChange={e => onRunsChange(Number(e.target.value))}
            className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
          />
        </div>
        <div>
          <label htmlFor="wl-concurrency" className="text-xs text-gray-500 mb-1 block">Concurrency</label>
          <input
            id="wl-concurrency"
            type="number"
            min={1}
            value={concurrency}
            onChange={e => onConcurrencyChange(Number(e.target.value))}
            className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
          />
        </div>
        <div>
          <label htmlFor="wl-timeout" className="text-xs text-gray-500 mb-1 block">Timeout (ms)</label>
          <input
            id="wl-timeout"
            type="number"
            min={100}
            value={timeoutMs}
            onChange={e => onTimeoutChange(Number(e.target.value))}
            className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
          />
        </div>
      </div>

      {hasThroughput && (
        <div>
          <label className="text-xs text-gray-500 mb-2 block">Payload Sizes</label>
          <PayloadSelector selected={selectedPayloads} onToggle={handlePayloadToggle} />
        </div>
      )}

      {showAdvanced && (
        <details className="border border-gray-800">
          <summary className="px-4 py-3 text-xs font-semibold text-gray-400 uppercase tracking-wider cursor-pointer hover:text-gray-300 transition-colors select-none">
            Advanced
          </summary>
          <div className="px-4 pb-4 space-y-3">
            <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
              <input type="checkbox" checked={insecure} onChange={e => onInsecureChange?.(e.target.checked)} className="accent-cyan-500" />
              Allow insecure HTTPS (skip TLS verification)
            </label>
            <label className="flex items-center gap-2 text-sm text-gray-300 cursor-pointer">
              <input type="checkbox" checked={connectionReuse} onChange={e => onConnectionReuseChange?.(e.target.checked)} className="accent-cyan-500" />
              Reuse connections (keep-alive)
            </label>
            <div>
              <label htmlFor="wl-capture-mode" className="block text-xs text-gray-400 mb-1">Capture mode</label>
              <select
                id="wl-capture-mode"
                value={captureMode}
                onChange={e => onCaptureModeChange?.(e.target.value as typeof captureMode)}
                className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
              >
                <option value="none">None</option>
                <option value="tester">Tester-side</option>
                <option value="endpoint">Endpoint-side</option>
                <option value="both">Both</option>
              </select>
            </div>
          </div>
        </details>
      )}
    </div>
  );
}
