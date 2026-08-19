import { useState } from 'react';
import { useNavigate } from 'react-router';
import { runsApi, type ComparisonLaunchResult } from '../../features/runs/api';
import type { Workload, Methodology, TestConfigCreate, ComparisonCell, ComparisonGroupCreate } from '../../api/types';
import { useToast, type ToastType } from '../../hooks/useToast';

/**
 * Toast for a comparison-group launch, from what the server ACTUALLY did.
 * The old code toasted `Launched ${cells.length} runs` from the requested
 * count, so a fully-failed launch showed a success toast (#793 P1-3 / #791).
 */
export function launchOutcomeToast(res: ComparisonLaunchResult): { type: ToastType; message: string } {
  const errors = res.errors ?? [];
  const detail = errors.length > 0 ? ` — ${errors.join('; ')}` : '';
  if (res.launched === 0) {
    return { type: 'error', message: `Launch failed: 0 of ${res.total} runs launched${detail}` };
  }
  if (res.failed > 0) {
    return { type: 'info', message: `Launched ${res.launched} of ${res.total} runs — ${res.failed} failed${detail}` };
  }
  return { type: 'success', message: `Launched ${res.launched} run${res.launched === 1 ? '' : 's'}` };
}

/**
 * The one submit path for the benchmark wizards (FullStack + Application —
 * both carried a byte-similar copy of this whole flow before the UI-4
 * merge): matrix runs become a launched comparison group; single-cell runs
 * become a TestConfig that can be saved, scheduled, and/or launched.
 */
interface ComparisonSubmitArgs {
  projectId: string;
  buildCells: () => ComparisonCell[];
  buildWorkload: () => Workload;
  methodology: Methodology;
  effectiveName: () => string;
  addSchedule: boolean;
  cronExpr: string;
  selectedTesterId: string | null;
  isMatrixRun: boolean;
  /** Toast shown when no cell could be built from the current selection. */
  emptyCellsError: string;
}

export function useComparisonSubmit({
  projectId,
  buildCells,
  buildWorkload,
  methodology,
  effectiveName,
  addSchedule,
  cronExpr,
  selectedTesterId,
  isMatrixRun,
  emptyCellsError,
}: ComparisonSubmitArgs) {
  const navigate = useNavigate();
  const addToast = useToast();
  const [submitting, setSubmitting] = useState(false);

  const handleSubmit = async (launchNow: boolean) => {
    setSubmitting(true);
    try {
      const name = effectiveName();
      const workload = buildWorkload();

      if (isMatrixRun && launchNow) {
        const cells = buildCells();
        const body: ComparisonGroupCreate = {
          name,
          base_workload: workload,
          methodology,
          cells,
        };
        const group = await runsApi.createComparisonGroup(projectId, body);
        const result = await runsApi.launchComparisonGroup(group.id);
        const toast = launchOutcomeToast(result);
        addToast(toast.type, toast.message);
        navigate(`/projects/${projectId}/runs?comparison_group=${group.id}`);
        return;
      }

      // Single-cell path: save/launch one TestConfig with a Pending endpoint.
      const cells = buildCells();
      const onlyEndpoint = cells[0]?.endpoint;
      if (!onlyEndpoint) {
        addToast('error', emptyCellsError);
        return;
      }
      const config: TestConfigCreate = {
        name,
        test_kind: 'benchmark',
        endpoint: onlyEndpoint,
        workload,
        methodology,
      };

      const created = await runsApi.createConfig(projectId, config);

      if (addSchedule) {
        await runsApi.createSchedule(projectId, {
          test_config_id: created.id,
          cron_expr: cronExpr,
        });
      }

      if (launchNow) {
        const run = await runsApi.launchConfig(created.id, selectedTesterId ?? undefined);
        addToast('success', `Run ${run.id.slice(0, 8)} launched`);
        navigate(`/projects/${projectId}/runs/${run.id}`);
      } else {
        addToast('success', `Config "${name}" saved`);
        navigate(`/projects/${projectId}/runs`);
      }
    } catch (e) {
      addToast('error', `Failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  return { submitting, handleSubmit };
}
