import type { LiveAttempt, RunEnvelope, RunInfra, RunInfraSide } from '../api/types';
import {
  assessRun,
  formatMbps,
  verdictLabel,
  wouldBenefitFromCeilingProbe,
  type DirectionAssessment,
} from '../lib/infra';
import { adviseRun } from '../lib/advisor';
import { formatBytes } from '../lib/analysis';

/**
 * Infrastructure envelope — what the run's hardware could do vs what it did.
 *
 * Two parts, both data-gated:
 *  - identity lines: runner/target VM size, cloud/region, vCPU/RAM and the
 *    catalog egress expectation (`est` = provider does not guarantee the size's
 *    bandwidth, `doc` = provider size table).
 *  - per-direction bars: steady-state measured (largest payload p50) against
 *    the sending side's egress ceiling, with the bottleneck verdict. A
 *    network-bound direction with an idle runner CPU is the "paying for CPU
 *    you can't feed" case the panel exists to surface.
 */
export function InfraEnvelope({
  infra,
  attempts,
  envelope,
}: {
  infra: RunInfra | null;
  attempts: LiveAttempt[];
  envelope?: RunEnvelope | null;
}) {
  if (!infra || (!infra.runner && !infra.target)) return null;

  const assessments = assessRun(attempts, infra, envelope);
  const suggestions = adviseRun(assessments, infra);

  const cores = envelope?.client_info?.cpu_cores;
  const peakLoad = Math.max(
    envelope?.client_load_before?.load_avg_1m ?? 0,
    envelope?.client_load_after?.load_avg_1m ?? 0
  );
  const cpuIdle =
    cores != null && cores > 0 && peakLoad > 0 && peakLoad < cores * 0.5;
  const anyNetworkBound = assessments.some((a) => a.verdict === 'network-bound');

  return (
    <div className="mb-6 border border-gray-800 rounded bg-[var(--bg-card)]">
      <h3 className="px-4 py-2.5 text-xs text-gray-400 tracking-wider bg-[var(--bg-surface)] border-b border-gray-800/50 font-medium">
        infrastructure envelope
      </h3>
      <div className="px-4 py-3 space-y-1.5">
        <SideLine label="runner" side={infra.runner} />
        <SideLine label="target" side={infra.target} />
      </div>

      {assessments.length > 0 && (
        <div className="px-4 pb-3 space-y-2">
          {assessments.map((a) => (
            <DirectionRow key={a.direction} a={a} />
          ))}
          {cpuIdle && anyNetworkBound && (
            <p className="text-[10px] text-gray-500 pt-1">
              runner CPU stayed idle (peak load {peakLoad.toFixed(2)} / {cores}{' '}
              cores) while a direction sat at its egress cap — the network, not
              compute, is the binding constraint of this infrastructure.
            </p>
          )}
          {suggestions.length > 0 && (
            <div className="pt-1 space-y-1">
              {suggestions.map((s) => (
                <div
                  key={`${s.side}:${s.to}`}
                  className="flex items-baseline gap-2 text-[11px] font-mono"
                >
                  <span
                    className={`shrink-0 px-1.5 py-0.5 rounded-sm border text-[10px] ${
                      s.kind === 'upsize'
                        ? 'border-cyan-600/60 bg-cyan-900/30 text-cyan-300'
                        : 'border-green-700/60 bg-green-900/20 text-green-300'
                    }`}
                  >
                    {s.kind}
                  </span>
                  <span className="text-gray-400">{s.text}</span>
                </div>
              ))}
              <p className="text-[10px] text-gray-600">
                prices: db cost_rates → curated cloud-costs table (list,
                on-demand) — deltas are hourly, verify in your billing before
                acting.
              </p>
            </div>
          )}
          {wouldBenefitFromCeilingProbe(assessments) && (
            <p className="text-[10px] text-cyan-600/80 pt-1">
              ceiling is an estimate — add the <span className="text-cyan-400">Multi-Conn
              (mthroughput)</span> mode to this config to measure the path&apos;s true
              multi-stream capacity; the envelope then uses the measured ceiling instead.
            </p>
          )}
          <p className="text-[10px] text-gray-600">
            expected = sending side&apos;s egress expectation per the VM-size
            catalog (doc = provider size table · est = size&apos;s bandwidth not
            guaranteed by the provider), superseded by the measured multi-stream
            capacity when the run includes mthroughput. steady-state = p50 at
            the largest payload.
          </p>
        </div>
      )}
    </div>
  );
}

function SideLine({ label, side }: { label: string; side: RunInfraSide | null }) {
  if (!side) return null;
  const s = side.specs;
  return (
    <div className="flex items-baseline gap-3 text-xs font-mono">
      <span className="w-14 text-right text-gray-500 shrink-0">{label}</span>
      <span className="text-gray-300">{side.vm_size ?? 'unknown size'}</span>
      <span className="text-gray-500">
        {side.cloud}
        {side.region ? ` ${side.region}` : ''}
      </span>
      {s ? (
        <span className="text-gray-500">
          {s.vcpus} vCPU / {s.memory_gb} GB ·{' '}
          <span className="text-gray-400">
            {s.confidence === 'estimated' ? '~' : ''}
            {formatMbps(s.egress_mbps)} egress
          </span>{' '}
          <span className="text-gray-600">
            ({s.confidence === 'documented' ? 'doc' : 'est'})
          </span>
          {!s.accelerated_networking && (
            <span className="text-gray-600"> · no accel-net</span>
          )}
        </span>
      ) : (
        <span className="text-gray-600">no spec in catalog</span>
      )}
    </div>
  );
}

const VERDICT_CHIP: Record<DirectionAssessment['verdict'], string> = {
  'network-bound': 'border-cyan-600/60 bg-cyan-900/30 text-cyan-300',
  'cpu-bound': 'border-yellow-600/60 bg-yellow-900/30 text-yellow-300',
  'path-bound': 'border-red-600/60 bg-red-900/30 text-red-300',
  headroom: 'border-gray-600/60 bg-gray-800/40 text-gray-300',
  unknown: 'border-gray-700/60 bg-gray-800/30 text-gray-500',
};

function DirectionRow({ a }: { a: DirectionAssessment }) {
  const pct =
    a.utilization != null ? Math.min(100, a.utilization * 100) : null;
  return (
    <div className="flex items-center gap-3 text-xs font-mono">
      <span className="w-14 text-right text-gray-400 shrink-0">
        {a.direction}
      </span>
      <div className="flex-1 relative h-3 rounded-sm bg-gray-800/60 overflow-hidden">
        {pct != null && (
          <div
            className="absolute inset-y-0 left-0 bg-cyan-600/70"
            style={{ width: `${pct}%` }}
          />
        )}
      </div>
      <span className="w-56 text-gray-400 shrink-0">
        {formatMbps(a.measuredMbps)}
        {a.expectedMbps != null && (
          <>
            {' / '}
            <span
              className={
                a.confidence === 'measured' ? 'text-cyan-400' : 'text-gray-500'
              }
            >
              {a.confidence === 'estimated' ? '~' : ''}
              {formatMbps(a.expectedMbps)}
            </span>
            {a.utilization != null && (
              <span className="text-gray-500">
                {' '}
                · {Math.round(a.utilization * 100)}%
              </span>
            )}
          </>
        )}
        <span className="text-gray-600"> @ {formatBytes(a.payloadBytes)}</span>
      </span>
      <span
        className={`shrink-0 px-1.5 py-0.5 rounded-sm border text-[10px] ${VERDICT_CHIP[a.verdict]}`}
        title={verdictLabel(a)}
      >
        {a.verdict}
      </span>
    </div>
  );
}
