import type { RunEnvelope } from '../api/types';
import { geoLabel } from '../lib/geo';

/**
 * Run-envelope context line: client/target geo, SNTP clock offset, and tester
 * load before/after — the run-scoped context the tester attaches to its final
 * TestRun JSON, served through the api/v2 run detail's `envelope` field
 * (V046). Entirely data-gated: renders nothing for old runs (no envelope) and
 * nothing for an envelope none of whose displayed fields are present.
 *
 * The noisy-tester warning fires when the 1-minute load average exceeded the
 * tester's core count (from `client_info.cpu_cores`) on either sample —
 * latency numbers measured from a contended host deserve suspicion.
 */
export function RunEnvelopeBlock({ envelope }: { envelope?: RunEnvelope | null }) {
  if (!envelope) return null;

  const fromLabel = geoLabel(envelope.client_geo);
  const toLabel = geoLabel(envelope.target_geo);
  const offsetMs = envelope.clock_sync?.offset_ms;
  const loadBefore = envelope.client_load_before?.load_avg_1m;
  const loadAfter = envelope.client_load_after?.load_avg_1m;
  const cpuCores = envelope.client_info?.cpu_cores;

  const hasLoad = loadBefore != null || loadAfter != null;
  const peakLoad = Math.max(loadBefore ?? 0, loadAfter ?? 0);
  const noisy = hasLoad && cpuCores != null && cpuCores > 0 && peakLoad > cpuCores;

  if (!fromLabel && !toLabel && offsetMs == null && !hasLoad) return null;

  return (
    <div className="flex flex-wrap gap-x-3 gap-y-0.5 mt-0.5">
      {fromLabel && (
        <span className="text-xs text-gray-600">
          From: <span className="text-gray-400 font-mono">{fromLabel}</span>
        </span>
      )}
      {toLabel && (
        <span className="text-xs text-gray-600">
          To: <span className="text-gray-400 font-mono">{toLabel}</span>
        </span>
      )}
      {offsetMs != null && (
        <span className="text-xs text-gray-600">
          Clock offset: <span className="text-gray-400 font-mono">{offsetMs > 0 ? '+' : ''}{offsetMs.toFixed(1)}ms</span>
        </span>
      )}
      {hasLoad && (
        <span className="text-xs text-gray-600">
          Tester load: <span className="text-gray-400 font-mono">
            {loadBefore?.toFixed(2) ?? '?'}
            {' → '}
            {loadAfter?.toFixed(2) ?? '?'}
          </span>
          {cpuCores != null && (
            <span className="text-gray-700 ml-1">({cpuCores} cores)</span>
          )}
        </span>
      )}
      {noisy && (
        <span className="text-xs text-yellow-400">
          &#9888; tester contended — load exceeded {cpuCores} cores; timings may be noisy
        </span>
      )}
    </div>
  );
}
