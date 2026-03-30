export function formatBenchmarkCount(value: number): string {
  return new Intl.NumberFormat().format(value);
}

export function formatBenchmarkNumber(value: number | null | undefined, maximumFractionDigits = 2): string {
  if (!Number.isFinite(value)) return '-';
  const finiteValue = value as number;
  const minimumFractionDigits = maximumFractionDigits === 0 ? 0 : Math.min(2, maximumFractionDigits);
  return new Intl.NumberFormat(undefined, {
    maximumFractionDigits,
    minimumFractionDigits,
  }).format(finiteValue);
}

export function formatBenchmarkMetric(value: number | null | undefined, unit: string): string {
  if (!Number.isFinite(value)) return '-';
  return `${formatBenchmarkNumber(value)} ${unit}`;
}

export function formatBenchmarkInterval(
  lower: number | null | undefined,
  upper: number | null | undefined,
  unit: string,
): string {
  if (!Number.isFinite(lower) || !Number.isFinite(upper)) return '-';
  return `${formatBenchmarkNumber(lower)} to ${formatBenchmarkNumber(upper)} ${unit}`;
}

export function formatBenchmarkDelta(value: number | null | undefined): string {
  if (!Number.isFinite(value)) return '-';
  const finiteValue = value as number;
  return `${finiteValue >= 0 ? '+' : ''}${formatBenchmarkNumber(finiteValue, 1)}%`;
}

export function formatBenchmarkRatio(value: number | null | undefined): string {
  if (!Number.isFinite(value) || value === 0) return '-';
  return `${formatBenchmarkNumber(value)}x`;
}

export function formatBenchmarkCaseLabel(caseInfo: {
  protocol: string;
  payload_bytes?: number | null;
  http_stack?: string | null;
}): string {
  const parts = [caseInfo.protocol.toUpperCase()];
  if (caseInfo.http_stack) parts.push(caseInfo.http_stack);
  if (caseInfo.payload_bytes != null) parts.push(`${formatBenchmarkCount(caseInfo.payload_bytes)} B`);
  return parts.join(' / ');
}
