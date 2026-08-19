import { useMemo, useState } from 'react';
import { Link, useParams } from 'react-router';
import { useProject } from '../hooks/useProject';
import { usePageTitle } from '../hooks/usePageTitle';
import { errorMessage } from '../api/client';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { PageShell } from '../components/common/PageShell';
import { StatusBadge } from '../components/common/StatusBadge';
import { runDisplayStatus } from '../lib/runStatus';
import { timeAgo } from '../lib/format';
import { formatMs, successRateClass } from '../lib/analysis';
import { formatBenchmarkMetric } from '../lib/benchmark';
import { languageColor } from '../lib/languageColors';
import {
  HorizontalBoxWhiskerChart,
  type HBoxGroup,
} from '../components/charts/HorizontalBoxWhiskerChart';
import {
  useComparisonGroupQuery,
  useRunsArtifactsQueries,
  useRunsAttemptsQueries,
  useTestRunsQuery,
} from '../features/runs/queries';
import {
  buildCaseMatrix,
  buildCellResult,
  groupByEnvironment,
  groupByLanguage,
  type CellResult,
  type EnvSection,
  type LanguageSection,
} from '../features/runs/compare';
import type { RunStatus } from '../api/types';

type Pivot = 'environment' | 'language';

// ── Small shared pieces ───────────────────────────────────────────────────────

function cellDisplayName(cell: CellResult): string {
  return cell.meta.language ?? cell.cellLabel;
}

function cellColor(cell: CellResult): string {
  return languageColor(cell.meta.language ?? cell.cellLabel);
}

function boxGroupsFor(cells: CellResult[], label: (c: CellResult) => string): HBoxGroup[] {
  const groups: HBoxGroup[] = [];
  for (const cell of cells) {
    const t = cell.stats.total;
    if (!t) continue;
    groups.push({
      label: label(cell),
      color: cellColor(cell),
      p5: t.p5,
      p25: t.p25,
      p50: t.p50,
      p75: t.p75,
      p95: t.p95,
      mean: t.mean,
    });
  }
  return groups;
}

function SuccessRateCell({ rate }: { rate: number | null }) {
  if (rate === null) return <span className="text-faint">-</span>;
  return <span className={successRateClass(rate)}>{rate.toFixed(0)}%</span>;
}

function RunLink({ projectId, cell }: { projectId: string; cell: CellResult }) {
  return (
    <Link
      to={`/projects/${projectId}/runs/${cell.run.id}`}
      className="text-cyan-400 hover:text-cyan-300 whitespace-nowrap"
    >
      run {cell.run.id.slice(0, 8)} &rarr;
    </Link>
  );
}

function StatusOrStats({ cell }: { cell: CellResult }) {
  if (cell.stats.total) return null;
  return <StatusBadge status={runDisplayStatus(cell.run)} />;
}

const thClass = 'py-2 pr-4 font-medium';
const sectionHeadingClass = 'text-sm font-semibold text-gray-300 uppercase tracking-wider';

// ── Per-case matrix (post-#796 artifact depth) ────────────────────────────────

function CaseMatrixTable({ projectId, cells }: { projectId: string; cells: CellResult[] }) {
  const matrix = useMemo(() => buildCaseMatrix(cells), [cells]);
  const missing = cells.filter((c) => !matrix.columns.some((col) => col.cell === c));

  if (matrix.columns.length === 0) {
    return (
      <p className="text-xs text-faint">
        Per-case breakdown unavailable for {cells.length === 1 ? 'this run' : 'these runs'} —
        showing attempt-derived stats only.
      </p>
    );
  }

  return (
    <div className="space-y-1">
      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-700 text-gray-400 text-left">
              <th className={thClass}>Case</th>
              {matrix.columns.map((col) => (
                <th key={col.cell.run.id} className={`${thClass} text-right`}>
                  <span style={{ color: cellColor(col.cell) }}>{cellDisplayName(col.cell)}</span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {matrix.caseLabels.map((caseLabel) => {
              const stats = matrix.columns.map((col) => col.byCase.get(caseLabel) ?? null);
              const comparable = stats.filter((s) => s !== null);
              const higherIsBetter = comparable[0]?.higherIsBetter ?? false;
              const bestP50 =
                comparable.length > 1
                  ? (higherIsBetter ? Math.max : Math.min)(...comparable.map((s) => s!.p50))
                  : null;
              return (
                <tr key={caseLabel} className="border-b border-gray-800 text-gray-300">
                  <td className="py-2 pr-4 font-mono text-xs">{caseLabel}</td>
                  {stats.map((s, i) => (
                    <td
                      key={matrix.columns[i].cell.run.id}
                      className={`py-2 pr-4 text-right ${s && s.p50 === bestP50 ? 'text-cyan-300' : ''}`}
                    >
                      {s ? (
                        <>
                          <span>{formatBenchmarkMetric(s.p50, s.unit)}</span>
                          <span className="text-faint text-xs ml-1.5">
                            p95 {formatBenchmarkMetric(s.p95, s.unit)}
                          </span>
                        </>
                      ) : (
                        <span className="text-faint">-</span>
                      )}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {missing.length > 0 && (
        <p className="text-xs text-faint">
          Per-case breakdown unavailable for{' '}
          {missing.map((cell, i) => (
            <span key={cell.run.id}>
              {i > 0 && ', '}
              <RunLink projectId={projectId} cell={cell} />
            </span>
          ))}
          .
        </p>
      )}
    </div>
  );
}

// ── Pivot 1: environment sections ─────────────────────────────────────────────

function EnvironmentSection({ projectId, section }: { projectId: string; section: EnvSection }) {
  const groups = useMemo(
    () => boxGroupsFor(section.cells, cellDisplayName),
    [section.cells],
  );
  const fastest = section.cells.find((c) => c.stats.total);

  return (
    <section className="space-y-3">
      <h2 className={sectionHeadingClass}>
        {section.label}
        <span className="ml-2 text-faint normal-case tracking-normal font-normal">
          {section.cells.length} cell{section.cells.length !== 1 ? 's' : ''}
        </span>
      </h2>

      {groups.length > 0 && (
        <HorizontalBoxWhiskerChart groups={groups} unit="ms" title="HTTP total duration" />
      )}

      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-700 text-gray-400 text-left">
              <th className={thClass}>#</th>
              <th className={thClass}>Language / cell</th>
              <th className={`${thClass} text-right`}>p50 &#9650;</th>
              <th className={`${thClass} text-right`}>p95</th>
              <th className={`${thClass} text-right`}>Success</th>
              <th className={`${thClass} text-right`}>Samples</th>
              <th className={thClass}>Run</th>
            </tr>
          </thead>
          <tbody>
            {section.cells.map((cell, i) => {
              const isFastest = cell === fastest;
              return (
                <tr key={cell.run.id} className="border-b border-gray-800 text-gray-300">
                  <td className="py-2 pr-4 text-faint">{i + 1}</td>
                  <td className="py-2 pr-4">
                    <span style={{ color: cellColor(cell) }}>{cellDisplayName(cell)}</span>
                    {isFastest && (
                      <span className="ml-2 text-xs text-cyan-300 border border-cyan-500/30 rounded px-1 py-0.5">
                        fastest
                      </span>
                    )}
                  </td>
                  <td className={`py-2 pr-4 text-right ${isFastest ? 'text-cyan-300' : ''}`}>
                    {cell.stats.total ? formatMs(cell.stats.total.p50) : <StatusOrStats cell={cell} />}
                  </td>
                  <td className="py-2 pr-4 text-right">
                    {cell.stats.total ? formatMs(cell.stats.total.p95) : '-'}
                  </td>
                  <td className="py-2 pr-4 text-right">
                    <SuccessRateCell rate={cell.stats.successRate} />
                  </td>
                  <td className="py-2 pr-4 text-right">{cell.stats.samples || '-'}</td>
                  <td className="py-2 pr-4">
                    <RunLink projectId={projectId} cell={cell} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <CaseMatrixTable projectId={projectId} cells={section.cells} />
    </section>
  );
}

// ── Pivot 2: language sections ────────────────────────────────────────────────

function LanguagePivotSection({
  projectId,
  section,
}: {
  projectId: string;
  section: LanguageSection;
}) {
  const language = section.language || 'unlabeled cells';
  const color = languageColor(section.language || 'unknown');
  const groups = useMemo(
    () =>
      boxGroupsFor(
        section.rows.map((r) => r.cell),
        (cell) => section.rows.find((r) => r.cell === cell)?.variantLabel ?? cell.cellLabel,
      ),
    [section.rows],
  );

  return (
    <section className="space-y-3">
      <h2 className={`${sectionHeadingClass} flex items-center gap-2 flex-wrap`}>
        <span
          className="inline-block w-2 h-2 rounded-full"
          style={{ backgroundColor: color }}
          aria-hidden="true"
        />
        <span style={{ color }}>{language}</span>
        {/* Whitespace-only text nodes render no flex box but DO join the
            heading's accessible name with a space. */}
        {' '}
        {section.differingAxes.length === 1 && (
          <span className="text-faint normal-case tracking-normal font-normal">
            varying: {section.differingAxes[0]}
          </span>
        )}
        {section.multiVariable && (
          <span className="text-xs font-normal normal-case tracking-normal text-yellow-400 border border-yellow-500/30 rounded px-1.5 py-0.5">
            multiple variables differ ({section.differingAxes.join(' · ')})
          </span>
        )}
      </h2>

      {groups.length > 1 && (
        <HorizontalBoxWhiskerChart groups={groups} unit="ms" title="HTTP total duration" />
      )}

      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b border-gray-700 text-gray-400 text-left">
              <th className={thClass}>Environment</th>
              <th className={`${thClass} text-right`}>p50 &#9650;</th>
              <th className={`${thClass} text-right`}>&Delta; vs fastest</th>
              <th className={`${thClass} text-right`}>p95</th>
              <th className={`${thClass} text-right`}>Success</th>
              <th className={`${thClass} text-right`}>Samples</th>
              <th className={thClass}>Run</th>
            </tr>
          </thead>
          <tbody>
            {section.rows.map((row) => {
              const { cell } = row;
              const isFastest = row.deltaPct !== null && row.deltaPct === 0;
              return (
                <tr key={cell.run.id} className="border-b border-gray-800 text-gray-300">
                  <td className="py-2 pr-4">{row.variantLabel}</td>
                  <td className={`py-2 pr-4 text-right ${isFastest ? 'text-cyan-300' : ''}`}>
                    {cell.stats.total ? formatMs(cell.stats.total.p50) : <StatusOrStats cell={cell} />}
                  </td>
                  <td className="py-2 pr-4 text-right">
                    {row.deltaPct === null ? (
                      <span className="text-faint">-</span>
                    ) : isFastest ? (
                      <span className="text-cyan-300">fastest</span>
                    ) : (
                      <span className="text-yellow-400">+{row.deltaPct.toFixed(1)}%</span>
                    )}
                  </td>
                  <td className="py-2 pr-4 text-right">
                    {cell.stats.total ? formatMs(cell.stats.total.p95) : '-'}
                  </td>
                  <td className="py-2 pr-4 text-right">
                    <SuccessRateCell rate={cell.stats.successRate} />
                  </td>
                  <td className="py-2 pr-4 text-right">{cell.stats.samples || '-'}</td>
                  <td className="py-2 pr-4">
                    <RunLink projectId={projectId} cell={cell} />
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}

// ── Page ──────────────────────────────────────────────────────────────────────

const STATUS_ORDER: RunStatus[] = ['completed', 'running', 'provisioning', 'queued', 'failed', 'cancelled'];

export function ComparisonResultsPage() {
  const { projectId } = useProject();
  const { groupId } = useParams<{ groupId: string }>();
  const gid = groupId ?? '';
  const shortId = gid.slice(0, 8);
  const [pivot, setPivot] = useState<Pivot>('environment');

  const groupQuery = useComparisonGroupQuery(gid);
  const group = groupQuery.data ?? null;
  usePageTitle(group ? `Compare: ${group.name}` : `Compare ${shortId}`);

  const runsQuery = useTestRunsQuery(projectId, { comparison_group_id: gid });
  const runs = useMemo(() => runsQuery.data ?? [], [runsQuery.data]);
  const runIds = useMemo(() => runs.map((r) => r.id), [runs]);
  const attemptsQueries = useRunsAttemptsQueries(runIds);
  const artifactQueries = useRunsArtifactsQueries(runs);

  const attemptsLoading = attemptsQueries.some((q) => q.isLoading);

  // Computed per render (no useMemo): useQueries returns fresh arrays every
  // render, so a dependency list would never be stable anyway, and a group is
  // at most a few dozen cells × a few hundred attempts — microseconds.
  const cells = runs.map((run, i) =>
    buildCellResult(
      run,
      attemptsQueries[i]?.data ?? [],
      artifactQueries[i]?.data ?? null,
      group?.cells,
    ),
  );

  const envSections = groupByEnvironment(cells);
  const langSections = groupByLanguage(cells);
  const hasLanguages = langSections.some((s) => s.language !== '');

  const statusCounts = useMemo(() => {
    const counts = new Map<RunStatus, number>();
    for (const run of runs) counts.set(run.status, (counts.get(run.status) ?? 0) + 1);
    return STATUS_ORDER.filter((s) => counts.has(s)).map((s) => ({ status: s, count: counts.get(s)! }));
  }, [runs]);

  const launchedAt = group?.created_at ?? runs[runs.length - 1]?.created_at ?? null;

  const breadcrumb = (
    <Breadcrumb
      items={[
        { label: 'Runs', to: `/projects/${projectId}/runs` },
        { label: `Compare ${shortId}` },
      ]}
    />
  );

  if (runsQuery.isPending) {
    return (
      <PageShell before={breadcrumb} title={`Compare ${shortId}`}>
        <div className="text-sm text-gray-400 motion-safe:animate-pulse py-8">
          Loading comparison group runs...
        </div>
      </PageShell>
    );
  }

  if (runsQuery.error) {
    return (
      <PageShell before={breadcrumb} title={`Compare ${shortId}`}>
        <div className="text-sm text-red-400 py-8">{errorMessage(runsQuery.error)}</div>
      </PageShell>
    );
  }

  return (
    <PageShell before={breadcrumb}>
      {/* Header */}
      <div className="mb-6 space-y-1">
        <div className="flex items-center gap-3 flex-wrap">
          <h2 className="text-xl font-bold text-gray-100">
            {group?.name ?? `Comparison group ${shortId}`}
          </h2>
          {statusCounts.map(({ status, count }) => (
            <StatusBadge key={status} status={status} label={`${count} ${status}`} />
          ))}
        </div>
        <p className="text-sm text-gray-400">
          {runs.length} cell{runs.length !== 1 ? 's' : ''}
          {group && group.cells.length !== runs.length && <> ({group.cells.length} defined)</>}
          {' · '}group <span className="font-mono">{shortId}</span>
          {launchedAt && <> · launched {timeAgo(launchedAt)}</>}
          {attemptsLoading && (
            <span className="text-faint motion-safe:animate-pulse"> · loading attempts...</span>
          )}
        </p>
      </div>

      {runs.length === 0 && (
        <div className="text-sm text-gray-400 py-8">
          No runs found for this comparison group.{' '}
          <Link to={`/projects/${projectId}/runs`} className="text-cyan-400 hover:text-cyan-300">
            Back to runs
          </Link>
        </div>
      )}

      {runs.length > 0 && (
        <>
          {/* Pivot tabs */}
          <div className="border-b border-gray-700 mb-6">
            <nav className="flex gap-1 -mb-px" aria-label="Comparison pivot">
              {(
                [
                  ['environment', 'By testbed'],
                  ['language', 'By language'],
                ] as [Pivot, string][]
              ).map(([value, label]) => (
                <button
                  key={value}
                  onClick={() => setPivot(value)}
                  aria-pressed={pivot === value}
                  className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
                    pivot === value
                      ? 'border-cyan-400 text-cyan-400'
                      : 'border-transparent text-gray-400 hover:text-gray-300 hover:border-gray-600'
                  }`}
                >
                  {label}
                </button>
              ))}
            </nav>
          </div>

          {pivot === 'environment' && (
            <div className="space-y-10">
              {envSections.map((section) => (
                <EnvironmentSection key={section.key} projectId={projectId} section={section} />
              ))}
            </div>
          )}

          {pivot === 'language' && (
            <div className="space-y-10">
              {!hasLanguages && (
                <p className="text-sm text-gray-400">
                  These cells carry no language axis (full-stack matrix) — each section below
                  groups the cells that share a label.
                </p>
              )}
              {langSections.map((section) => (
                <LanguagePivotSection
                  key={section.language || '(none)'}
                  projectId={projectId}
                  section={section}
                />
              ))}
            </div>
          )}
        </>
      )}
    </PageShell>
  );
}
