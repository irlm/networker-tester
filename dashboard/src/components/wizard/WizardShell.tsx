import type { ReactNode } from 'react';
import { Breadcrumb } from '../common/Breadcrumb';
import { WizardStepper } from './WizardStepper';

/**
 * The shared page chrome of the benchmark wizards: breadcrumb, title,
 * stepper, and the Back/Next footer (with the "why is Next disabled" hint —
 * a silently grey button was audit §8). Step content is the caller's.
 */
interface WizardShellProps {
  breadcrumbLabel: string;
  breadcrumbTo: string;
  title: string;
  subtitle: string;
  steps: string[];
  step: number;
  onStepClick: (step: number) => void;
  /** Steps (by index) that render their own primary action — no Next. */
  hideNextOn?: number[];
  canNext: boolean;
  nextHint?: string | null;
  onNext: () => void;
  onBack: () => void;
  children: ReactNode;
}

export function WizardShell({
  breadcrumbLabel,
  breadcrumbTo,
  title,
  subtitle,
  steps,
  step,
  onStepClick,
  hideNextOn = [],
  canNext,
  nextHint,
  onNext,
  onBack,
  children,
}: WizardShellProps) {
  const lastStep = steps.length - 1;
  const showNext = step < lastStep && !hideNextOn.includes(step);

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <Breadcrumb items={[{ label: breadcrumbLabel, to: breadcrumbTo }, { label: 'New Benchmark' }]} />

      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">{title}</h2>
        <p className="text-xs text-gray-400 mt-1">{subtitle}</p>
      </div>

      <WizardStepper steps={steps} currentStep={step} onStepClick={onStepClick} />

      {children}

      <div className="flex items-center justify-between mt-10 pt-4 border-t border-gray-800/50">
        <button
          onClick={onBack}
          disabled={step === 0}
          className="text-xs text-gray-400 disabled:text-gray-700 disabled:cursor-not-allowed hover:text-gray-300 transition-colors"
        >
          Back
        </button>
        {showNext && (
          <div className="flex items-center gap-3">
            {nextHint && (
              <span className="text-xs text-gray-400">{nextHint}</span>
            )}
            <button
              onClick={onNext}
              disabled={!canNext}
              className="px-5 py-2 bg-cyan-600 hover:bg-cyan-500 disabled:bg-gray-800 disabled:text-gray-600 text-[var(--bg-base)] text-xs font-medium transition-colors"
            >
              Next
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
