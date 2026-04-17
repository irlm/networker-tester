import { useState } from 'react';
import type { Methodology } from '../../api/types';
import { METHODOLOGY_PRESETS } from './testbed-constants';

export interface MethodologyPanelProps {
  /** Whether the user can toggle benchmark mode off. For FullStack/AppBenchmark this is always true. */
  alwaysOn?: boolean;
  benchmarkMode: boolean;
  onBenchmarkModeChange: (on: boolean) => void;
  methodology: Methodology;
  onMethodologyChange: (m: Methodology) => void;
  methodPreset: string;
  onMethodPresetChange: (presetId: string) => void;
}

export function MethodologyPanel({
  alwaysOn,
  benchmarkMode,
  onBenchmarkModeChange,
  methodology,
  onMethodologyChange,
  methodPreset,
  onMethodPresetChange,
}: MethodologyPanelProps) {
  const [showAdvanced, setShowAdvanced] = useState(false);

  const applyPreset = (presetId: string) => {
    const p = METHODOLOGY_PRESETS.find(m => m.id === presetId);
    if (!p) return;
    onMethodPresetChange(presetId);
    // A preset with targetError == null means "no error target"; force to 0
    // so switching Rigorous → Quick doesn't leave a stale 2% gate active.
    onMethodologyChange({
      ...methodology,
      warmup_runs: p.warmup,
      measured_runs: p.measured,
      target_error_pct: p.targetError ?? 0,
    });
  };

  return (
    <div>
      <h3 className="text-sm font-semibold text-gray-200 mb-4">Methodology</h3>

      {!alwaysOn && (
        <label className="flex items-center gap-3 cursor-pointer mb-6">
          <input
            type="checkbox"
            checked={benchmarkMode}
            onChange={e => onBenchmarkModeChange(e.target.checked)}
            className="w-4 h-4 border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
          />
          <div>
            <span className="text-sm text-gray-200">Enable benchmark mode</span>
            <p className="text-xs text-gray-500">Adds warmup, measured iterations, quality gates, and generates a publishable artifact.</p>
          </div>
        </label>
      )}

      {alwaysOn && (
        <p className="text-xs text-gray-500 mb-6">
          Benchmark methodology is always enabled for this test type. Configure warmup, measured iterations, and quality gates below.
        </p>
      )}

      {(benchmarkMode || alwaysOn) && (
        <>
          {/* Presets */}
          <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-6">
            {METHODOLOGY_PRESETS.map(p => (
              <button
                key={p.id}
                onClick={() => applyPreset(p.id)}
                className={`text-left border p-4 transition-colors ${
                  methodPreset === p.id
                    ? 'border-cyan-500/50 bg-cyan-500/5'
                    : 'border-gray-800 hover:border-gray-600'
                }`}
              >
                <h4 className="text-sm font-medium text-gray-100">{p.label}</h4>
                <div className="text-xs text-gray-500 mt-2 space-y-1">
                  <div>{p.warmup} warmup runs</div>
                  <div>{p.measured} measured runs</div>
                  <div>{p.targetError != null ? `${p.targetError}% target error` : 'No error target'}</div>
                </div>
              </button>
            ))}
          </div>

          {/* Advanced toggle */}
          <button
            onClick={() => setShowAdvanced(!showAdvanced)}
            className="text-xs text-gray-400 hover:text-gray-200 transition-colors mb-4"
          >
            {showAdvanced ? 'Hide' : 'Show'} advanced options
          </button>

          {showAdvanced && (
            <div className="border border-gray-800 p-4 space-y-4">
              <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
                <label className="text-xs text-gray-500">
                  Warmup runs
                  <input
                    type="number"
                    value={methodology.warmup_runs}
                    onChange={e => { onMethodologyChange({ ...methodology, warmup_runs: Number(e.target.value) }); onMethodPresetChange('custom'); }}
                    min={0}
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </label>
                <label className="text-xs text-gray-500">
                  Measured runs
                  <input
                    type="number"
                    value={methodology.measured_runs}
                    onChange={e => { onMethodologyChange({ ...methodology, measured_runs: Number(e.target.value) }); onMethodPresetChange('custom'); }}
                    min={1}
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </label>
                <label className="text-xs text-gray-500">
                  Cooldown (ms)
                  <input
                    type="number"
                    value={methodology.cooldown_ms}
                    onChange={e => { onMethodologyChange({ ...methodology, cooldown_ms: Number(e.target.value) }); onMethodPresetChange('custom'); }}
                    min={0}
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </label>
                <label className="text-xs text-gray-500">
                  Target error %
                  <input
                    type="number"
                    value={methodology.target_error_pct}
                    onChange={e => { onMethodologyChange({ ...methodology, target_error_pct: Number(e.target.value) }); onMethodPresetChange('custom'); }}
                    min={0}
                    step={0.5}
                    className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500"
                  />
                </label>
              </div>
              <div className="text-xs text-gray-600 pt-2">
                Quality gates: CV &lt; {methodology.quality_gates.max_cv_pct}%, min {methodology.quality_gates.min_samples} samples.
                Publication gate: failure rate &lt; {methodology.publication_gates.max_failure_pct}%.
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
