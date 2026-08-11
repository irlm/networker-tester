import type { ReactNode } from 'react';
import type { Methodology } from '../../api/types';
import type { TestbedState } from './testbed-constants';
import { PROXY_LABELS, TESTER_OS_OPTIONS } from './testbed-constants';

/**
 * The shared Review & Launch step of the benchmark wizards. Both wizards
 * carried a near-identical copy that had already drifted (win/linux tag
 * colors, matrix-note hue); this is the canonical form — neutral OS tags,
 * cyan matrix note (purple is reserved for the brand/runner semantic).
 *
 * Wizard-specific review rows (template, languages, provisioning notice)
 * come in through `extraSections` / `beforeSchedule`.
 */
interface ReviewStepProps {
  configName: string;
  onConfigNameChange: (name: string) => void;
  namePlaceholder: string;
  /** The compact one-line summary under the name field. */
  summaryLine: ReactNode;
  /** Rows rendered between the summary and the testbed list. */
  extraSections?: ReactNode;
  testbeds: TestbedState[];
  /** Rows rendered after the workload summary (languages, notices). */
  afterWorkload?: ReactNode;
  methodology: Methodology;
  workloadLine: ReactNode;
  matrixNote?: ReactNode;
  addSchedule: boolean;
  onAddScheduleChange: (on: boolean) => void;
  cronExpr: string;
  onCronExprChange: (expr: string) => void;
  isMatrixRun: boolean;
  submitting: boolean;
  onSubmit: (launchNow: boolean) => void;
  launchLabel: string;
}

export function ReviewStep({
  configName,
  onConfigNameChange,
  namePlaceholder,
  summaryLine,
  extraSections,
  testbeds,
  afterWorkload,
  methodology,
  workloadLine,
  matrixNote,
  addSchedule,
  onAddScheduleChange,
  cronExpr,
  onCronExprChange,
  isMatrixRun,
  submitting,
  onSubmit,
  launchLabel,
}: ReviewStepProps) {
  return (
    <div>
      <h3 className="text-sm font-semibold text-gray-200 mb-4">Review & Launch</h3>

      <label className="text-xs text-gray-400 block mb-4">
        Benchmark name
        <input
          type="text"
          value={configName}
          onChange={e => onConfigNameChange(e.target.value)}
          placeholder={namePlaceholder}
          className="mt-1 w-full bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
        />
      </label>

      <div className="text-xs text-gray-400 mb-4">{summaryLine}</div>

      {extraSections}

      {/* Testbeds */}
      <div className="mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1.5">Testbeds</div>
        <div className="space-y-0.5">
          {testbeds.map((testbed, idx) => (
            <div key={testbed.key} className="flex items-center gap-2 text-xs py-1 border-b border-gray-800/50 last:border-0">
              <span className="text-gray-400 w-4">{idx + 1}</span>
              <span className="text-gray-200">{testbed.cloud}</span>
              <span className="text-gray-400">/</span>
              <span className="text-gray-300">{testbed.region}</span>
              <span className="text-[10px] px-1 text-gray-300">
                {testbed.os === 'windows' ? 'win' : 'linux'}
              </span>
              <span className="text-gray-500">{testbed.vmSize}</span>
              <span className="text-gray-700">{testbed.topology}</span>
              <span className="text-cyan-500/70">{testbed.proxies.map(p => PROXY_LABELS[p] ?? p).join(', ')}</span>
              <span className="text-gray-500">{TESTER_OS_OPTIONS.find(o => o.id === testbed.testerOs)?.label ?? testbed.testerOs}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Methodology */}
      <div className="mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1.5">Methodology</div>
        <div className="text-xs text-gray-400">
          {methodology.warmup_runs} warmup / {methodology.measured_runs} measured / {methodology.target_error_pct > 0 ? `${methodology.target_error_pct}% target error` : 'no error target'}
        </div>
      </div>

      {/* Workload */}
      <div className="mb-4">
        <div className="text-[10px] uppercase tracking-wider text-gray-500 mb-1.5">Workload</div>
        <div className="text-xs text-gray-400">{workloadLine}</div>
      </div>

      {matrixNote && (
        <div className="text-xs text-cyan-400 mb-4">{matrixNote}</div>
      )}

      {afterWorkload}

      {/* Schedule */}
      <label className="flex items-center gap-3 cursor-pointer mb-4">
        <input
          type="checkbox"
          checked={addSchedule}
          onChange={e => onAddScheduleChange(e.target.checked)}
          className="w-4 h-4 border-gray-600 bg-gray-900 text-cyan-500 focus:ring-cyan-500/50"
        />
        <span className="text-sm text-gray-200">Add schedule</span>
      </label>

      {addSchedule && (
        <div className="border border-gray-800 p-4 mb-4">
          <label htmlFor="cron" className="text-xs text-gray-400 mb-1 block">Cron Expression (6-field)</label>
          <input
            id="cron"
            type="text"
            value={cronExpr}
            onChange={e => onCronExprChange(e.target.value)}
            className="bg-[var(--bg-base)] border border-gray-700 px-3 py-2 text-sm text-gray-200 w-full focus:outline-none focus:border-cyan-500"
          />
          <p className="text-xs text-gray-500 mt-1">sec min hour day month weekday -- e.g. 0 0 * * * * = hourly</p>
        </div>
      )}

      {/* Launch buttons — matrix runs can't be saved as a single config
          (the save path would silently keep only the first cell). */}
      <div className="flex gap-2">
        {!isMatrixRun && (
          <button
            onClick={() => onSubmit(false)}
            disabled={submitting}
            className="border border-gray-700 hover:border-gray-600 text-gray-300 px-4 py-2 text-sm transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
          >
            Save Config
          </button>
        )}
        <button
          onClick={() => onSubmit(true)}
          disabled={submitting}
          className={`text-white px-6 py-2.5 text-sm font-medium transition-colors disabled:cursor-wait ${
            submitting ? 'bg-cyan-700 cursor-wait' : 'bg-cyan-600 hover:bg-cyan-500'
          }`}
        >
          {submitting ? (
            <span className="flex items-center gap-2">
              <span className="inline-block w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
              Launching...
            </span>
          ) : launchLabel}
        </button>
      </div>
    </div>
  );
}
