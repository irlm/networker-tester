import { Link } from 'react-router';
import { buttonClassName } from '../../../components/common/button-styles';
import type { Scenario } from '../../../lib/scenarios';
import {
  SCENARIO_INTENTS,
  scenarioAvailability,
  scenarioMethod,
  scenarioOutput,
  type ScenarioIntentId,
} from '../model';
import type { ScenarioReadinessResponse } from '../api';

const TONE_CLASS = {
  ready: 'text-green-400',
  attention: 'text-yellow-400',
  blocked: 'text-red-400',
  unverified: 'text-gray-400',
} as const;

interface IntentSelectorProps {
  active: ScenarioIntentId;
  onChange: (intent: ScenarioIntentId) => void;
}

export function IntentSelector({ active, onChange }: IntentSelectorProps) {
  return (
    <section className="mt-7" aria-labelledby="intent-heading">
      <div className="mb-3 flex items-baseline justify-between gap-3">
        <h3 id="intent-heading" className="text-sm font-semibold text-gray-200">What signal do you need?</h3>
        <span className="hidden text-xs text-faint sm:inline">Keys 1–4</span>
      </div>
      <div className="grid grid-cols-2 border-l border-t border-[var(--border-default)] lg:grid-cols-4" role="tablist" aria-label="Test intent">
        {SCENARIO_INTENTS.map((intent, index) => {
          const selected = intent.id === active;
          return (
            <button
              key={intent.id}
              type="button"
              role="tab"
              aria-selected={selected}
              data-intent={intent.id}
              data-intent-active={selected}
              onClick={() => onChange(intent.id)}
              className={`min-h-16 border-b border-r border-[var(--border-default)] px-3 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-cyan-400 ${
                selected ? '!border-b-cyan-400 bg-[var(--bg-surface)] text-cyan-300' : 'text-gray-400 hover:bg-gray-900/60 hover:text-gray-100'
              }`}
            >
              <span className={`mr-2 text-xs ${selected ? 'text-cyan-200' : 'text-faint'}`}>{index + 1}</span>
              <span className="text-xs font-semibold sm:text-sm">{intent.label}</span>
            </button>
          );
        })}
      </div>
      <p className="mt-2 max-w-3xl text-xs leading-relaxed text-gray-400">
        {SCENARIO_INTENTS.find((intent) => intent.id === active)?.description}
      </p>
    </section>
  );
}

interface ScenarioConsoleProps {
  scenarios: Scenario[];
  projectId: string;
  readiness?: ScenarioReadinessResponse;
  selectedIndex: number;
  methodOpen: boolean;
  onToggleMethod: () => void;
}

export function ScenarioConsole({
  scenarios,
  projectId,
  readiness,
  selectedIndex,
  methodOpen,
  onToggleMethod,
}: ScenarioConsoleProps) {
  const [recommended, ...alternatives] = scenarios;
  if (!recommended) return null;
  const availability = scenarioAvailability(recommended, readiness);

  return (
    <div className="mt-7">
      <section aria-labelledby="recommended-heading">
        <div className="mb-3 flex items-center justify-between gap-3">
          <h3 id="recommended-heading" className="text-sm font-semibold text-gray-200">Recommended now</h3>
          <span className="text-xs text-faint">Ranked by readiness and time to signal</span>
        </div>
        <article
          data-scenario-id={recommended.id}
          className={`border bg-[var(--bg-surface)] ${selectedIndex === 0 ? 'border-cyan-500/80' : 'border-cyan-500/45'}`}
        >
          <div className="border-b border-cyan-500/25 px-4 py-2 text-xs text-cyan-300">
            RECOMMENDED · {recommended.badge}
          </div>
          <div className="grid gap-5 p-4 lg:grid-cols-[minmax(18rem,1.5fr)_minmax(8rem,.55fr)_minmax(10rem,.65fr)_minmax(16rem,1fr)] lg:items-start">
            <div>
              <h4 className="text-base font-bold text-gray-100">{recommended.title}</h4>
              <p className="mt-1 max-w-2xl text-xs leading-relaxed text-gray-400">{recommended.summary}</p>
              <div className="mt-3 flex flex-wrap gap-1.5">
                {recommended.measures.map((measure) => (
                  <span key={measure} className="border border-gray-800 bg-[var(--bg-base)] px-1.5 py-0.5 text-xs text-gray-400">
                    {measure}
                  </span>
                ))}
              </div>
            </div>
            <Metric label="Time to signal" value={recommended.est} />
            <Metric label="Readiness" value={availability.label} valueClass={TONE_CLASS[availability.tone]} detail={availability.detail} />
            <div>
              <p className="section-label mb-1">Output</p>
              <p className="text-xs leading-relaxed text-gray-300">{scenarioOutput(recommended)}</p>
              <Link
                data-scenario-action
                className={buttonClassName({ variant: availability.canConfigure ? 'primary' : 'secondary', size: 'sm', className: 'mt-4 w-full sm:w-auto' })}
                to={availability.actionPath(projectId)}
              >
                {availability.actionLabel} →
              </Link>
            </div>
          </div>
          <button
            type="button"
            className="flex w-full items-center justify-between border-t border-[var(--border-default)] px-4 py-2 text-left text-xs text-gray-400 hover:bg-gray-900/40 hover:text-gray-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-cyan-400"
            onClick={onToggleMethod}
            aria-expanded={methodOpen}
          >
            <span>Methodology and launch behavior</span>
            <span aria-hidden="true">{methodOpen ? '−' : '+'}</span>
          </button>
          {methodOpen && (
            <p className="border-t border-[var(--border-default)] px-4 py-3 text-xs leading-relaxed text-gray-400">
              {scenarioMethod(recommended)}
            </p>
          )}
        </article>
      </section>

      {alternatives.length > 0 && (
        <section className="mt-7" aria-labelledby="alternative-heading">
          <h3 id="alternative-heading" className="mb-3 text-sm font-semibold text-gray-200">Other tests for this signal</h3>
          <div className="border-t border-[var(--border-default)]">
            {alternatives.map((scenario, index) => (
              <ScenarioRow
                key={scenario.id}
                scenario={scenario}
                projectId={projectId}
                readiness={readiness}
                selected={selectedIndex === index + 1}
              />
            ))}
          </div>
        </section>
      )}
    </div>
  );
}

function Metric({ label, value, valueClass = 'text-gray-100', detail }: { label: string; value: string; valueClass?: string; detail?: string }) {
  return (
    <div>
      <p className="section-label mb-1">{label}</p>
      <p className={`text-sm font-bold ${valueClass}`}>{value}</p>
      {detail && <p className="mt-1 text-xs leading-relaxed text-gray-400">{detail}</p>}
    </div>
  );
}

function ScenarioRow({ scenario, projectId, readiness, selected }: {
  scenario: Scenario;
  projectId: string;
  readiness?: ScenarioReadinessResponse;
  selected: boolean;
}) {
  const availability = scenarioAvailability(scenario, readiness);
  return (
    <article
      data-scenario-id={scenario.id}
      className={`grid gap-3 border-b border-[var(--border-default)] px-3 py-4 transition-colors md:grid-cols-[minmax(15rem,1.2fr)_minmax(7rem,.45fr)_minmax(14rem,1fr)_auto] md:items-center ${
        selected ? 'bg-cyan-500/5' : 'hover:bg-gray-900/30'
      }`}
    >
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <h4 className="text-sm font-semibold text-gray-100">{scenario.title}</h4>
          <span className="border border-gray-800 px-1.5 py-0.5 text-xs text-faint">{scenario.badge}</span>
        </div>
        <p className="mt-1 text-xs leading-relaxed text-gray-400">{scenario.summary}</p>
      </div>
      <div className="flex items-center justify-between gap-3 md:block">
        <span className="section-label md:block">Time</span>
        <span className="text-xs font-semibold text-gray-300">{scenario.est}</span>
      </div>
      <div className="min-w-0">
        <span className={`text-xs font-semibold ${TONE_CLASS[availability.tone]}`}>{availability.label}</span>
        <p className="mt-1 text-xs leading-relaxed text-gray-400">{scenarioOutput(scenario)}</p>
      </div>
      <Link
        data-scenario-action
        className={buttonClassName({ variant: 'secondary', size: 'xs', className: 'w-full md:w-auto' })}
        to={availability.actionPath(projectId)}
      >
        {availability.actionLabel} →
      </Link>
    </article>
  );
}
