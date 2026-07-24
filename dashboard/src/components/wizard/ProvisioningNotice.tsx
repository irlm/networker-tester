import { provisioningSummary, type ProvisioningNoticeProps } from './provisioning-notice';

// Cost + runner-readiness notice shown on a benchmark review step before launch.
// A `pending` endpoint provisions real cloud VMs at LAUNCH (never at create), so
// this makes spend explicit right before the launch button and surfaces the
// runner gap. Logic + types live in ./provisioning-notice.ts.

export function ProvisioningNotice(props: ProvisioningNoticeProps) {
  if (props.vmCount <= 0) return null;
  const { headline, runner, runnerOk } = provisioningSummary(props);
  return (
    <div className="border border-amber-500/40 bg-amber-500/5 p-3 mb-4">
      <div className="text-xs font-mono text-amber-300">⚠ {headline}</div>
      <div className={`mt-1 text-xs font-mono ${runnerOk ? 'text-gray-400' : 'text-amber-400'}`}>
        {runnerOk ? '✓ ' : '⚠ '}
        {runner}
      </div>
    </div>
  );
}
