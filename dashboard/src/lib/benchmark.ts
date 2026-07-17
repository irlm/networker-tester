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

export function formatBenchmarkDelta(value: number | null | undefined): string {
  if (!Number.isFinite(value)) return '-';
  const finiteValue = value as number;
  return `${finiteValue >= 0 ? '+' : ''}${formatBenchmarkNumber(finiteValue, 1)}%`;
}
