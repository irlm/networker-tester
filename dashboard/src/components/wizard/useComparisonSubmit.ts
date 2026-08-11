import { useState } from 'react';
import { useNavigate } from 'react-router';
import { api } from '../../api/client';
import type { Workload, Methodology, TestConfigCreate, ComparisonCell, ComparisonGroupCreate } from '../../api/types';
import { useToast } from '../../hooks/useToast';

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
        const group = await api.createComparisonGroup(projectId, body);
        await api.launchComparisonGroup(group.id);
        addToast('success', `Launched ${cells.length} run${cells.length === 1 ? '' : 's'}`);
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
        endpoint: onlyEndpoint,
        workload,
        methodology,
      };

      const created = await api.createTestConfig(projectId, config);

      if (addSchedule) {
        await api.createTestSchedule(projectId, {
          test_config_id: created.id,
          cron_expr: cronExpr,
        });
      }

      if (launchNow) {
        const run = await api.launchTestConfig(created.id, selectedTesterId ?? undefined);
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
