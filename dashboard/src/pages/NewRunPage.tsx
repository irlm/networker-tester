import { Link } from 'react-router-dom';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';

const TEST_TYPES = [
  {
    title: 'Network Test',
    description: 'Test raw network protocol performance against a deployed endpoint or hostname. All 18 modes, no methodology overhead.',
    path: 'tests/new',
    steps: 'Target / Workload / Review',
    accent: 'border-cyan-500/40',
  },
  {
    title: 'Full Stack Benchmark',
    description: 'Test infrastructure stack performance through proxies with statistical methodology. Testbed matrix across clouds, regions, and proxy configs.',
    path: 'benchmarks/full-stack/new',
    steps: 'Testbeds / Workload / Methodology / Review',
    accent: 'border-purple-500/40',
  },
  {
    title: 'Application Benchmark',
    description: 'Compare language and framework performance. Template gallery, language selection, and full statistical methodology.',
    path: 'benchmarks/application/new',
    steps: 'Template / Testbeds / Languages / Methodology / Review',
    accent: 'border-green-500/40',
  },
] as const;

export function NewRunPage() {
  const { projectId } = useProject();
  usePageTitle('New Run');

  return (
    <div className="p-4 md:p-6 max-w-4xl">
      <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'New' }]} />

      <div className="mb-8">
        <h2 className="text-lg md:text-xl font-bold text-gray-100">Choose Test Type</h2>
        <p className="text-xs text-gray-500 mt-1">
          Select the kind of test you want to run.
        </p>
      </div>

      <div className="grid gap-3">
        {TEST_TYPES.map(t => (
          <Link
            key={t.path}
            to={`/projects/${projectId}/${t.path}`}
            className={`block border ${t.accent} bg-gray-900/30 p-5 hover:bg-gray-800/40 transition-colors group`}
          >
            <div className="flex items-start justify-between">
              <div>
                <h3 className="text-sm font-semibold text-gray-100 group-hover:text-cyan-200 transition-colors">
                  {t.title}
                </h3>
                <p className="text-xs text-gray-500 mt-1 max-w-xl">
                  {t.description}
                </p>
              </div>
              <span className="text-gray-600 group-hover:text-gray-400 transition-colors text-lg ml-4">
                {'\u2192'}
              </span>
            </div>
            <div className="text-[10px] font-mono text-gray-600 mt-3">
              {t.steps}
            </div>
          </Link>
        ))}
      </div>
    </div>
  );
}
