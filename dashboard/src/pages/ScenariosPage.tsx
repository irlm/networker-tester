import { Link } from 'react-router-dom';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { SCENARIO_GROUPS, type Scenario } from '../lib/scenarios';

// "Start from a scenario" launcher. Each card drops the user into one of the
// four existing test flows, pre-filled from the scenario's preset/modes — so
// they pick an outcome instead of hand-building a config. Everything is still
// editable on the destination page before launch (config-only prefill; no
// provisioning happens here). See lib/scenarios.ts.

function ScenarioCard({ scenario, projectId }: { scenario: Scenario; projectId: string }) {
  return (
    <Link
      to={scenario.href(projectId)}
      className="group flex flex-col border border-gray-800 bg-[var(--bg-base)] hover:border-cyan-500/60 transition-colors p-4"
    >
      <div className="flex items-center justify-between mb-2">
        <span className="text-[10px] uppercase tracking-wider font-mono text-cyan-400 border border-cyan-500/30 px-1.5 py-0.5">
          {scenario.badge}
        </span>
        <span className="text-[11px] font-mono text-gray-500">{scenario.est}</span>
      </div>

      <h3 className="text-sm font-bold text-gray-100 group-hover:text-cyan-300 transition-colors">
        {scenario.title}
      </h3>
      <p className="mt-1 text-xs text-gray-400 leading-relaxed">{scenario.summary}</p>

      <div className="mt-3 flex flex-wrap gap-1">
        {scenario.measures.map((m) => (
          <span
            key={m}
            className="text-[10px] font-mono text-gray-400 bg-gray-800/60 border border-gray-800 px-1.5 py-0.5"
          >
            {m}
          </span>
        ))}
      </div>

      <div className="mt-3 pt-3 border-t border-gray-800/60 flex items-center justify-between">
        <span className="text-[11px] font-mono text-gray-500">
          <span className="text-gray-600">needs</span> {scenario.needs}
        </span>
        <span className="text-xs font-mono text-cyan-400 group-hover:translate-x-0.5 transition-transform">
          Configure →
        </span>
      </div>
    </Link>
  );
}

export function ScenariosPage() {
  usePageTitle('New Test');
  const { projectId } = useProject();

  return (
    <div className="p-4 md:p-6 max-w-5xl">
      <div className="mb-6">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Start a test</h2>
        <p className="mt-1 text-xs font-mono text-gray-400">
          Pick a scenario — we pre-fill the right test and modes. You can tweak everything before
          launching. Prefer to build from scratch? Every flow is still in the sidebar.
        </p>
      </div>

      <div className="space-y-8">
        {SCENARIO_GROUPS.map((group) => (
          <section key={group.id}>
            <div className="mb-3">
              <h3 className="text-xs uppercase tracking-wider font-mono text-gray-300">
                {group.label}
              </h3>
              <p className="text-[11px] font-mono text-gray-500">{group.blurb}</p>
            </div>
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              {group.scenarios.map((s) => (
                <ScenarioCard key={s.id} scenario={s} projectId={projectId} />
              ))}
            </div>
          </section>
        ))}
      </div>
    </div>
  );
}
