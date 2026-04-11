type Phase = 'queued' | 'starting' | 'deploy' | 'running' | 'collect' | 'done';
type Outcome = 'success' | 'partial_success' | 'failure' | 'cancelled';

interface PhaseBarProps {
  phase: Phase;
  outcome: Outcome | null;
  appliedStages: Phase[];
}

const PHASE_LABELS: Record<Phase, string> = {
  queued: 'Queued',
  starting: 'Starting',
  deploy: 'Deploy',
  running: 'Running',
  collect: 'Collect',
  done: 'Done',
};

const OUTCOME_COLOR: Record<Outcome, string> = {
  success: 'bg-emerald-600',
  partial_success: 'bg-amber-500',
  failure: 'bg-rose-600',
  cancelled: 'bg-gray-500',
};

export function PhaseBar({ phase, outcome, appliedStages }: PhaseBarProps) {
  const currentIndex = appliedStages.indexOf(phase);
  const isDone = phase === 'done';

  return (
    <div
      className="flex items-stretch gap-1"
      role="progressbar"
      aria-label={`phase: ${phase}`}
    >
      {appliedStages.map((stage, idx) => {
        const isLast = idx === appliedStages.length - 1;
        const isBefore = currentIndex >= 0 && idx < currentIndex;
        const isActive = idx === currentIndex && !isDone;
        const isLater = currentIndex >= 0 ? idx > currentIndex : !isDone;
        const isDoneLast = isDone && isLast;
        const isDoneEarlier = isDone && !isLast;

        let fillColor = 'bg-gray-800';
        if (isDoneLast) {
          fillColor = outcome ? OUTCOME_COLOR[outcome] : 'bg-cyan-600';
        } else if (isDoneEarlier || isBefore) {
          fillColor = 'bg-cyan-600';
        } else if (isActive) {
          fillColor = 'bg-purple-500 motion-safe:animate-pulse';
        } else if (isLater) {
          fillColor = 'bg-gray-800';
        }

        let labelColor = 'text-gray-500';
        if (isDoneLast) {
          labelColor = 'text-white font-semibold';
        } else if (isDoneEarlier || isBefore) {
          labelColor = 'text-cyan-400';
        } else if (isActive) {
          labelColor = 'text-white';
        }

        const fillClass = `h-2 rounded-sm transition-colors ${fillColor}`;
        const labelClass = `text-[10px] font-mono uppercase tracking-wide ${labelColor}`;

        return (
          <div
            key={stage}
            className="flex-1 flex flex-col gap-1"
            data-stage={stage}
          >
            <div className={fillClass} data-testid={`phase-segment-${stage}`} />
            <span className={labelClass}>{PHASE_LABELS[stage]}</span>
          </div>
        );
      })}
    </div>
  );
}
