export interface WizardStepperProps {
  steps: string[];
  currentStep: number;
  onStepClick: (step: number) => void;
}

export function WizardStepper({ steps, currentStep, onStepClick }: WizardStepperProps) {
  return (
    <div className="flex items-center gap-0.5 mb-8 font-mono text-xs">
      {steps.map((label, i) => {
        const isCurrent = i === currentStep;
        const isPast = i < currentStep;
        return (
          <button
            key={label}
            onClick={() => { if (isPast) onStepClick(i); }}
            disabled={!isPast && !isCurrent}
            className={`px-2 py-1 transition-colors ${
              isCurrent
                ? 'text-cyan-300'
                : isPast
                  ? 'text-gray-500 hover:text-gray-300 cursor-pointer'
                  : 'text-gray-700 cursor-not-allowed'
            }`}
          >
            {i + 1}. {label}
          </button>
        );
      })}
    </div>
  );
}
