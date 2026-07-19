// Pure form logic for the alerting UI (rule builder + channel editor).
// Mirrors the backend validation in AlertsEndpoints/AlertRuleLogic
// (docs/alerting.md) so obvious mistakes are caught before the round-trip.

import type { AlertChannelConfig, AlertChannelKind, AlertComparator, AlertMetric } from '../../api/types';

export const MIN_WINDOW_RUNS = 1;
export const MAX_WINDOW_RUNS = 50;

/** Placeholder the API returns instead of a webhook secret. PATCHing it back
 *  verbatim keeps the stored secret (write-only round-trip contract). */
export const SECRET_MASK = '********';

export interface AlertMetricInfo {
  value: AlertMetric;
  label: string;
  /** 'ms' metrics are latencies; 'ratio' metrics are 0..1 rates. */
  unit: 'ms' | 'ratio';
}

export const ALERT_METRICS: AlertMetricInfo[] = [
  { value: 'p95_ms', label: 'p95 latency', unit: 'ms' },
  { value: 'mean_ms', label: 'mean latency', unit: 'ms' },
  { value: 'error_rate', label: 'error rate', unit: 'ratio' },
  { value: 'success_rate', label: 'success rate', unit: 'ratio' },
];

export const ALERT_COMPARATORS: { value: AlertComparator; label: string }[] = [
  { value: 'gt', label: '> above' },
  { value: 'lt', label: '< below' },
];

export function metricUnit(metric: AlertMetric): 'ms' | 'ratio' {
  return ALERT_METRICS.find((m) => m.value === metric)?.unit ?? 'ms';
}

export function formatThreshold(metric: AlertMetric, threshold: number): string {
  return metricUnit(metric) === 'ms' ? `${threshold}ms` : String(threshold);
}

/** Terminal-style condition summary, e.g. `p95_ms > 500ms`. */
export function formatCondition(metric: AlertMetric, comparator: AlertComparator, threshold: number): string {
  return `${metric} ${comparator === 'gt' ? '>' : '<'} ${formatThreshold(metric, threshold)}`;
}

// ── Rule builder ────────────────────────────────────────────────────────────

export interface RuleFormValues {
  metric: AlertMetric;
  comparator: AlertComparator;
  /** Raw input strings so partially-typed values don't NaN mid-edit. */
  threshold: string;
  windowRuns: string;
  channelId: string;
  /** '' → project-wide (every config in the project). */
  testConfigId: string;
}

export function validateRuleForm(v: RuleFormValues): string | null {
  const threshold = Number(v.threshold);
  if (v.threshold.trim() === '' || !Number.isFinite(threshold)) {
    return 'Threshold must be a finite number';
  }
  if (metricUnit(v.metric) === 'ratio' && (threshold < 0 || threshold > 1)) {
    return 'Rate thresholds are ratios — use a value between 0 and 1';
  }
  const window = Number(v.windowRuns);
  if (v.windowRuns.trim() === '' || !Number.isInteger(window) || window < MIN_WINDOW_RUNS || window > MAX_WINDOW_RUNS) {
    return `Window must be between ${MIN_WINDOW_RUNS} and ${MAX_WINDOW_RUNS} consecutive runs`;
  }
  if (!v.channelId) {
    return 'Select a notification channel';
  }
  return null;
}

// ── Channel editor ──────────────────────────────────────────────────────────

export interface ChannelFormValues {
  kind: AlertChannelKind;
  name: string;
  /** webhook only */
  url: string;
  /** webhook only; write-only — prefilled with SECRET_MASK when one is stored. */
  secret: string;
  /** email only; comma/whitespace/semicolon-separated recipient list. */
  to: string;
}

export function parseRecipients(to: string): string[] {
  return to
    .split(/[\s,;]+/)
    .map((a) => a.trim())
    .filter((a) => a.length > 0);
}

export function validateChannelForm(v: ChannelFormValues): string | null {
  if (!v.name.trim()) {
    return 'Name is required';
  }
  if (v.kind === 'webhook') {
    let parsed: URL;
    try {
      parsed = new URL(v.url.trim());
    } catch {
      return 'Webhook URL must be an absolute http(s) URL';
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      return 'Webhook URL must be an absolute http(s) URL';
    }
    return null;
  }
  // email
  const recipients = parseRecipients(v.to);
  if (recipients.length === 0) {
    return 'Add at least one recipient address';
  }
  const bad = recipients.find((r) => !r.includes('@'));
  if (bad) {
    return `'${bad}' is not a valid email address`;
  }
  return null;
}

/** Build the config payload. An empty webhook secret omits the field
 *  (removing any stored secret); the untouched mask keeps it. */
export function channelConfigFromForm(v: ChannelFormValues): AlertChannelConfig {
  if (v.kind === 'webhook') {
    const secret = v.secret.trim();
    return secret ? { url: v.url.trim(), secret } : { url: v.url.trim() };
  }
  return { to: parseRecipients(v.to) };
}
