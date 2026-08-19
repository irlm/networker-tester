import { useCallback, useEffect, useMemo, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Link, useNavigate } from 'react-router';
import { Button } from '../components/common/Button';
import { PageShell } from '../components/common/PageShell';
import { StatusBadge } from '../components/common/StatusBadge';
import { buttonClassName } from '../components/common/button-styles';
import { KeyboardRail } from '../features/scenarios/components/KeyboardRail';
import { ReadinessStrip } from '../features/scenarios/components/ReadinessStrip';
import { IntentSelector, ScenarioConsole } from '../features/scenarios/components/ScenarioConsole';
import {
  rankScenarios,
  SCENARIO_INTENTS,
  summarizeReadiness,
  type ScenarioIntentId,
} from '../features/scenarios/model';
import { useScenarioReadinessQuery } from '../features/scenarios/queries';
import { useScenarioKeyboardNavigation } from '../features/scenarios/useScenarioKeyboardNavigation';
import { runsApi } from '../features/runs/api';
import { useTestRunsQuery } from '../features/runs/queries';
import { usePageTitle } from '../hooks/usePageTitle';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';
import { ALL_SCENARIOS } from '../lib/scenarios';

const KEYBOARD_STORAGE_KEY = 'scenario-keys-enabled';
const SCENARIOS_BY_ID = new Map(ALL_SCENARIOS.map((scenario) => [scenario.id, scenario]));

function rankedScenariosForIntent(intent: ScenarioIntentId, readiness?: Parameters<typeof rankScenarios>[1]) {
  const definition = SCENARIO_INTENTS.find((item) => item.id === intent) ?? SCENARIO_INTENTS[0];
  return rankScenarios(
    definition.scenarioIds.flatMap((id) => {
      const scenario = SCENARIOS_BY_ID.get(id);
      return scenario ? [scenario] : [];
    }),
    readiness,
  );
}

function initialKeyboardSetting(): boolean {
  if (typeof window === 'undefined') return true;
  return window.localStorage.getItem(KEYBOARD_STORAGE_KEY) !== 'false';
}

function formatRecentTime(iso: string): string {
  const elapsed = Math.max(0, Date.now() - new Date(iso).getTime());
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 1) return 'just now';
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function ScenariosPage() {
  usePageTitle('Start a test');
  const { projectId, isOperator } = useProject();
  const navigate = useNavigate();
  const addToast = useToast();
  const [intent, setIntentState] = useState<ScenarioIntentId>('url');
  const [readinessOpen, setReadinessOpen] = useState(false);
  const [methodOpen, setMethodOpen] = useState(false);
  const [keyboardEnabled, setKeyboardEnabled] = useState(initialKeyboardSetting);

  const readinessQuery = useScenarioReadinessQuery(projectId);
  const recentRunsQuery = useTestRunsQuery(projectId, { limit: 10 }, { polling: false });
  // Intentionally frozen between explicit user actions: the 15s readiness
  // poll may update labels, but it must never move the Configure target under
  // the pointer or silently change a keyboard selection.
  const [scenarioIds, setScenarioIds] = useState<string[]>(() =>
    rankedScenariosForIntent('url', readinessQuery.data).map((scenario) => scenario.id));
  const [selectedScenarioId, setSelectedScenarioId] = useState<string | null>(() => scenarioIds[0] ?? null);
  const readinessItems = useMemo(
    () => summarizeReadiness(readinessQuery.data),
    [readinessQuery.data],
  );

  const scenarios = useMemo(
    () => scenarioIds.flatMap((id) => {
      const scenario = SCENARIOS_BY_ID.get(id);
      return scenario ? [scenario] : [];
    }),
    [scenarioIds],
  );

  const setIntent = useCallback((nextIntent: ScenarioIntentId) => {
    const ranked = rankedScenariosForIntent(nextIntent, readinessQuery.data);
    setIntentState(nextIntent);
    setScenarioIds(ranked.map((scenario) => scenario.id));
    setSelectedScenarioId(ranked[0]?.id ?? null);
    setMethodOpen(false);
  }, [readinessQuery.data]);

  const retryReadiness = useCallback(async () => {
    const result = await readinessQuery.refetch();
    const ranked = rankedScenariosForIntent(intent, result?.data ?? readinessQuery.data);
    const nextIds = ranked.map((scenario) => scenario.id);
    setScenarioIds(nextIds);
    setSelectedScenarioId((selected) => selected && nextIds.includes(selected) ? selected : (nextIds[0] ?? null));
  }, [intent, readinessQuery]);

  const toggleMethod = useCallback(() => setMethodOpen((open) => !open), []);
  const toggleReadiness = useCallback(() => setReadinessOpen((open) => !open), []);

  useScenarioKeyboardNavigation({
    enabled: keyboardEnabled,
    activeIntent: intent,
    scenarioIds,
    selectedScenarioId,
    setSelectedScenarioId,
    setIntent,
    toggleMethod,
    toggleReadiness,
  });

  useEffect(() => {
    window.localStorage.setItem(KEYBOARD_STORAGE_KEY, String(keyboardEnabled));
  }, [keyboardEnabled]);

  const latestRun = recentRunsQuery.data?.find((run) =>
    run.status === 'completed' || run.status === 'failed' || run.status === 'cancelled');
  const canRerun = Boolean(
    latestRun && isOperator && latestRun.test_kind != null && latestRun.test_kind !== 'benchmark',
  );
  const rerunMutation = useMutation({
    mutationFn: (configId: string) => runsApi.launchConfig(configId),
    onSuccess: (run) => {
      addToast('success', 'Run started with the previous configuration.');
      navigate(`/projects/${projectId}/runs/${run.id}`);
    },
    onError: (error) => addToast('error', error instanceof Error ? error.message : 'Could not start the run.'),
  });

  return (
    <PageShell
      title="Start a test"
      subtitle="Choose the right test for the signal you need right now. Every option opens an editable configuration before launch."
      className="mx-auto max-w-[1440px] pb-0"
      action={(
        <span className="hidden text-xs text-faint lg:block">
          Press <kbd className="border border-gray-700 px-1.5 py-0.5 text-gray-300">/</kbd> to search every test
        </span>
      )}
    >
      <ReadinessStrip
        items={readinessItems}
        projectId={projectId}
        expanded={readinessOpen}
        onToggle={toggleReadiness}
        onRetry={() => void retryReadiness()}
        refreshing={readinessQuery.isFetching}
        checkedAt={readinessQuery.dataUpdatedAt || undefined}
      />

      <IntentSelector active={intent} onChange={setIntent} />
      <ScenarioConsole
        scenarios={scenarios}
        projectId={projectId}
        readiness={readinessQuery.data}
        selectedScenarioId={selectedScenarioId}
        methodOpen={methodOpen}
        onToggleMethod={toggleMethod}
      />

      <section className="section-divider" aria-labelledby="recent-heading">
        <div className="mb-3 flex items-center justify-between gap-3">
          <div>
            <h3 id="recent-heading" className="text-sm font-semibold text-gray-200">Resume recent work</h3>
            <p className="mt-1 text-xs text-gray-400">Open the last result, or safely repeat its saved configuration.</p>
          </div>
          <Link to={`/projects/${projectId}/runs`} className="text-xs text-cyan-400 hover:text-cyan-300">All runs →</Link>
        </div>

        {recentRunsQuery.isPending ? (
          <div className="border-y border-[var(--border-default)] px-3 py-4 text-xs text-gray-400" role="status">Loading recent runs…</div>
        ) : recentRunsQuery.isError ? (
          <div className="flex flex-wrap items-center justify-between gap-3 border-y border-red-500/30 px-3 py-4 text-xs text-red-300">
            <span>Recent runs could not be loaded. The test catalog is still available.</span>
            <Button size="xs" variant="secondary" onClick={() => void recentRunsQuery.refetch()}>Retry</Button>
          </div>
        ) : latestRun ? (
          <article className="grid gap-3 border-y border-[var(--border-default)] px-3 py-4 md:grid-cols-[minmax(14rem,1fr)_auto_auto] md:items-center">
            <div className="min-w-0">
              <div className="flex flex-wrap items-center gap-2">
                <h4 className="truncate text-sm font-semibold text-gray-100">{latestRun.config_name ?? 'Saved test configuration'}</h4>
                <StatusBadge status={latestRun.status} />
              </div>
              <p className="mt-1 text-xs text-gray-400">
                {latestRun.test_kind?.replace('_', ' ') ?? 'test'} · {formatRecentTime(latestRun.created_at)} · {latestRun.success_count} passed / {latestRun.failure_count} failed
              </p>
            </div>
            {latestRun.test_kind === 'benchmark' && isOperator && (
              <span className="text-xs text-yellow-400">Benchmark reruns require a fresh testbed review.</span>
            )}
            {canRerun ? (
              <Button
                size="xs"
                variant="primary"
                loading={rerunMutation.isPending}
                loadingLabel="Starting…"
                onClick={() => rerunMutation.mutate(latestRun.test_config_id)}
              >
                Run with last configuration
              </Button>
            ) : (
              <Link className={buttonClassName({ variant: 'secondary', size: 'xs' })} to={`/projects/${projectId}/runs/${latestRun.id}`}>
                Open result →
              </Link>
            )}
          </article>
        ) : (
          <div className="border-y border-[var(--border-default)] px-3 py-5">
            <p className="text-sm text-gray-300">No completed runs yet.</p>
            <p className="mt-1 text-xs text-gray-400">Choose an intent above; LagHound will preserve the configuration after your first run.</p>
          </div>
        )}
      </section>

      <KeyboardRail enabled={keyboardEnabled} onToggle={() => setKeyboardEnabled((enabled) => !enabled)} />
    </PageShell>
  );
}
