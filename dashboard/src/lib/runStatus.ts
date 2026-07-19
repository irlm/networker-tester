/**
 * The one verdict rule for a run, applied identically on Runs, Run detail,
 * Dashboard, and URL Probe (audit F9: the same run read green "completed" on
 * /runs and red "failed" on /probe).
 *
 * - completed with zero failures        → completed (green)
 * - completed with some failures + some
 *   successes                           → partial (amber)
 * - completed where everything failed   → failed (red)
 * - anything else                       → backend status verbatim
 */
export function runDisplayStatus(run: {
  status: string;
  success_count: number;
  failure_count: number;
}): string {
  if (run.status === 'completed' && run.failure_count > 0) {
    return run.success_count > 0 ? 'partial' : 'failed';
  }
  return run.status;
}
