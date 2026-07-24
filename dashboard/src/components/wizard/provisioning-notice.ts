// Pure helper + types for <ProvisioningNotice> — kept out of the .tsx so the
// component file exports only a component (react-refresh/only-export-components).

export interface ProvisioningNoticeProps {
  /** Endpoint VMs that will be provisioned (one per testbed; proxies co-locate). */
  vmCount: number;
  /** Cloud label, or 'multiple' when testbeds span clouds. */
  cloud: string;
  /** Region, or 'multiple' when testbeds span regions. Empty hides the region. */
  region: string;
  /** Online runners available to dispatch the run. */
  onlineRunners: number;
}

export interface ProvisioningSummary {
  headline: string;
  runner: string;
  runnerOk: boolean;
}

/** Derives the notice text — a benchmark's `pending` endpoint provisions real
 *  cloud VMs at LAUNCH (never at create); the run only dispatches once a runner
 *  is online. Unit-tested in ProvisioningNotice.test.ts. */
export function provisioningSummary(p: ProvisioningNoticeProps): ProvisioningSummary {
  const vms = `${p.vmCount} VM${p.vmCount === 1 ? '' : 's'}`;
  const where = p.region ? `${p.cloud} · ${p.region}` : p.cloud;
  const headline = `Launching provisions ${vms} on ${where}. Cloud charges apply until each VM's auto-shutdown.`;

  const runnerOk = p.onlineRunners > 0;
  const runner = runnerOk
    ? `${p.onlineRunners} runner${p.onlineRunners === 1 ? '' : 's'} online — the run dispatches immediately.`
    : 'No runner online — the target still provisions, but the run queues until a runner is available (add one from Infrastructure).';

  return { headline, runner, runnerOk };
}
