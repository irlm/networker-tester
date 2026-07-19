import { useEffect, useRef, useState } from 'react';
import { api } from '../../api/client';
import type { AlertChannel, AlertComparator, AlertMetric, AlertRule, TestConfigListItem } from '../../api/types';
import { useToast } from '../../hooks/useToast';
import {
  ALERT_COMPARATORS,
  ALERT_METRICS,
  MAX_WINDOW_RUNS,
  MIN_WINDOW_RUNS,
  metricUnit,
  validateRuleForm,
} from './alert-form';

interface RuleDialogProps {
  projectId: string;
  channels: AlertChannel[];
  configs: TestConfigListItem[];
  /** When set, the dialog edits this rule instead of creating one. */
  existing?: AlertRule | null;
  onClose: () => void;
  onSaved: () => void;
}

const inputCls =
  'w-full bg-[var(--bg-base)] border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-cyan-500';

export function RuleDialog({ projectId, channels, configs, existing, onClose, onSaved }: RuleDialogProps) {
  const [metric, setMetric] = useState<AlertMetric>(existing?.metric ?? 'p95_ms');
  const [comparator, setComparator] = useState<AlertComparator>(existing?.comparator ?? 'gt');
  const [threshold, setThreshold] = useState(existing != null ? String(existing.threshold) : '');
  const [windowRuns, setWindowRuns] = useState(existing != null ? String(existing.window_runs) : '1');
  const [channelId, setChannelId] = useState(existing?.channel_id ?? channels[0]?.channel_id ?? '');
  const [testConfigId, setTestConfigId] = useState(existing?.test_config_id ?? '');
  const [enabled, setEnabled] = useState(existing?.enabled ?? true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const firstInputRef = useRef<HTMLSelectElement>(null);
  const addToast = useToast();

  // A config-scoped rule cannot be widened back to project-wide via PATCH
  // (the backend cannot distinguish "clear" from "unchanged") — recreate it.
  const scopeLocked = existing != null && existing.test_config_id != null;

  useEffect(() => {
    firstInputRef.current?.focus();
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const validation = validateRuleForm({ metric, comparator, threshold, windowRuns, channelId, testConfigId });
    if (validation) {
      setError(validation);
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (existing) {
        await api.updateAlertRule(existing.rule_id, {
          metric,
          comparator,
          threshold: Number(threshold),
          window_runs: Number(windowRuns),
          channel_id: channelId,
          ...(testConfigId ? { test_config_id: testConfigId } : {}),
          enabled,
        });
        addToast('success', 'Alert rule updated');
      } else {
        await api.createAlertRule(projectId, {
          metric,
          comparator,
          threshold: Number(threshold),
          window_runs: Number(windowRuns),
          channel_id: channelId,
          ...(testConfigId ? { test_config_id: testConfigId } : {}),
          enabled,
        });
        addToast('success', 'Alert rule created');
      }
      onSaved();
      onClose();
    } catch (err) {
      const msg = err instanceof Error ? err.message : 'Failed to save alert rule';
      setError(msg);
    } finally {
      setSaving(false);
    }
  };

  const unit = metricUnit(metric);

  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="absolute inset-0 bg-black/40 slide-over-backdrop" onClick={onClose} aria-hidden="true" />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="alert-rule-dialog-title"
        className="relative w-full md:w-[520px] md:max-w-[90vw] bg-[var(--bg-base)] md:border-l border-gray-800 h-full overflow-y-auto slide-over-panel"
      >
        {/* noValidate: alert-form.ts owns validation so errors render in the
            styled banner instead of native constraint tooltips. */}
        <form onSubmit={handleSubmit} noValidate className="p-4 md:p-6">
          <div className="flex items-center justify-between mb-6">
            <h3 id="alert-rule-dialog-title" className="text-lg font-bold text-gray-100">
              {existing ? 'Edit Alert Rule' : 'New Alert Rule'}
            </h3>
            <button type="button" onClick={onClose} className="text-gray-500 hover:text-gray-300 text-sm" aria-label="Close">
              &#x2715;
            </button>
          </div>

          {error && (
            <div role="alert" className="bg-red-500/10 border border-red-500/30 rounded p-2 mb-4 text-red-400 text-sm">
              {error}
            </div>
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
            <div>
              <label htmlFor="rule-metric" className="block text-xs text-gray-400 mb-1">Metric</label>
              <select
                id="rule-metric"
                ref={firstInputRef}
                value={metric}
                onChange={(e) => setMetric(e.target.value as AlertMetric)}
                className={inputCls}
              >
                {ALERT_METRICS.map((m) => (
                  <option key={m.value} value={m.value}>
                    {m.value} — {m.label}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor="rule-comparator" className="block text-xs text-gray-400 mb-1">Comparator</label>
              <select
                id="rule-comparator"
                value={comparator}
                onChange={(e) => setComparator(e.target.value as AlertComparator)}
                className={inputCls}
              >
                {ALERT_COMPARATORS.map((c) => (
                  <option key={c.value} value={c.value}>{c.label}</option>
                ))}
              </select>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-4">
            <div>
              <label htmlFor="rule-threshold" className="block text-xs text-gray-400 mb-1">
                Threshold {unit === 'ms' ? '(ms)' : '(ratio 0..1)'}
              </label>
              <input
                id="rule-threshold"
                type="number"
                step="any"
                value={threshold}
                onChange={(e) => setThreshold(e.target.value)}
                placeholder={unit === 'ms' ? '500' : '0.05'}
                className={`${inputCls} font-mono`}
              />
            </div>
            <div>
              <label htmlFor="rule-window" className="block text-xs text-gray-400 mb-1">Consecutive runs</label>
              <input
                id="rule-window"
                type="number"
                min={MIN_WINDOW_RUNS}
                max={MAX_WINDOW_RUNS}
                value={windowRuns}
                onChange={(e) => setWindowRuns(e.target.value)}
                className={`${inputCls} font-mono`}
              />
            </div>
          </div>

          <div className="mb-4">
            <label htmlFor="rule-scope" className="block text-xs text-gray-400 mb-1">Scope</label>
            <select
              id="rule-scope"
              value={testConfigId}
              onChange={(e) => setTestConfigId(e.target.value)}
              className={inputCls}
            >
              <option value="" disabled={scopeLocked}>
                All configs in project
              </option>
              {configs.map((c) => (
                <option key={c.id} value={c.id}>{c.name}</option>
              ))}
            </select>
            {scopeLocked && (
              <p className="text-[11px] text-gray-600 mt-1">
                A config-scoped rule cannot be widened back to all configs — recreate it instead.
              </p>
            )}
          </div>

          <div className="mb-4">
            <label htmlFor="rule-channel" className="block text-xs text-gray-400 mb-1">Notify channel</label>
            <select
              id="rule-channel"
              value={channelId}
              onChange={(e) => setChannelId(e.target.value)}
              className={inputCls}
            >
              {channels.length === 0 && <option value="">No channels — create one first</option>}
              {channels.map((c) => (
                <option key={c.channel_id} value={c.channel_id}>
                  {c.name} ({c.kind})
                </option>
              ))}
            </select>
          </div>

          <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer mb-6">
            <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} className="accent-cyan-500" />
            Enabled
          </label>

          <div className="flex justify-end gap-3 pt-4 border-t border-gray-800/50 mt-6">
            <button type="button" onClick={onClose} className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200">
              Cancel
            </button>
            <button
              type="submit"
              disabled={saving}
              className="bg-cyan-600 hover:bg-cyan-500 text-white px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
            >
              {saving ? 'Saving...' : existing ? 'Save Rule' : 'Create Rule'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
