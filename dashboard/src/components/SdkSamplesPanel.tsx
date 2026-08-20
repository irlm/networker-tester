import { Link } from 'react-router';
import type { SdkSampleStatus } from '../api/types';
import { cloudProviderText } from '../lib/provider';
import { SAMPLE_ACTION_LABEL, SAMPLE_STATE_CHIP, versionLine } from '../lib/sdkSamples';
import { Button } from './common/Button';

interface SdkSamplesPanelProps {
  projectId: string;
  samples: SdkSampleStatus[];
  isOperator: boolean;
  /** Open the create dialog (optionally pre-selecting one language). */
  onDeploy: (language?: string) => void;
  /** Register an existing, usable sample as an SDK endpoint. */
  onReuse: (language: string) => void;
  /** Re-run the sample's deployment in place to pick up the current SDK. */
  onUpdate: (language: string) => void;
  /** Language currently being acted on (its button shows a spinner). */
  busy?: string | null;
}

/**
 * What this project has, per SDK sample language — and the one thing to do
 * about it.
 *
 * The page used to list the samples as brochure entries: you could read about
 * the C# app and open its source, but there was no way to get one. Every row
 * here carries a real state from the server (`SdkSamplePlan`), the deployed SDK
 * version next to the current one, where it runs and what it costs to keep, and
 * a single action:
 *
 *  - **nothing deployed** → Deploy
 *  - **deployed and current** → Reuse (register it; no new server)
 *  - **deployed but outdated** → Update in place, with both versions shown
 *  - **unreachable / failed deploy** → Redeploy; never presented as usable
 *  - **deploying** → nothing, with the deployment linked
 */
export function SdkSamplesPanel({
  projectId,
  samples,
  isOperator,
  onDeploy,
  onReuse,
  onUpdate,
  busy,
}: SdkSamplesPanelProps) {
  const deployed = samples.filter((s) => s.state !== 'none').length;
  const outdatedCount = samples.filter((s) => s.state === 'outdated').length;

  return (
    <section className="mb-6 overflow-hidden rounded-lg border border-gray-800" aria-labelledby="sdk-samples-title">
      <div className="flex flex-col gap-3 border-b border-gray-800 bg-[var(--bg-surface)] p-4 md:flex-row md:items-start md:justify-between">
        <div className="max-w-2xl">
          <h2 id="sdk-samples-title" className="text-sm font-bold text-gray-100">
            SDK samples
          </h2>
          <p className="mt-1 text-xs text-gray-400">
            The LagHound reference apps. Deploy them on one server or one per language, reuse what is
            already running, and update a sample when the SDK has moved on.
          </p>
          <p className="mt-1 text-xs text-faint">
            {deployed} of {samples.length} deployed
            {outdatedCount > 0 && (
              <span className="text-yellow-400"> · {outdatedCount} outdated</span>
            )}
          </p>
        </div>
        {isOperator && (
          <Button variant="primary" size="sm" onClick={() => onDeploy()}>
            Deploy samples
          </Button>
        )}
      </div>

      <div className="divide-y divide-gray-800">
        {samples.map((s) => {
          const chip = SAMPLE_STATE_CHIP[s.state];
          const isBusy = busy === s.language;
          return (
            <article
              key={s.language}
              className="grid gap-3 p-4 md:grid-cols-[minmax(0,1fr)_minmax(14rem,0.9fr)_auto] md:items-center"
            >
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <h3 className="text-sm font-semibold text-gray-200">{s.label}</h3>
                  <span className="text-xs text-faint">{s.runtime}</span>
                  <span className={`rounded border px-1.5 py-0.5 text-xs ${chip.className}`}>
                    {chip.label}
                  </span>
                </div>
                <p className="mt-1 text-xs text-gray-400">{s.reason}</p>
              </div>

              <div className="min-w-0 text-xs">
                <div className="text-gray-400">
                  <span className="text-faint">SDK </span>
                  <span className={s.state === 'outdated' ? 'text-yellow-400' : 'text-gray-300'}>
                    {versionLine(s)}
                  </span>
                  <span className="text-gray-600"> · :{s.port}</span>
                </div>
                {s.url && (
                  <div className="mt-0.5 break-all text-cyan-400 md:truncate" title={s.url}>
                    {s.url}
                  </div>
                )}
                {s.provider && (
                  <div className="mt-0.5 text-faint">
                    <span className={cloudProviderText(s.provider)}>{s.provider}</span>
                    {s.region ? ` ${s.region}` : ''}
                    {s.vm_size ? ` · ${s.vm_size}` : ''}
                    {s.samples_on_host && s.samples_on_host > 1
                      ? ` · shares this server with ${s.samples_on_host - 1} other sample${s.samples_on_host === 2 ? '' : 's'}`
                      : ''}
                  </div>
                )}
                {s.deployment_id && (
                  <Link
                    to={`/projects/${projectId}/deploy/${s.deployment_id}`}
                    className="mt-0.5 inline-block text-faint hover:text-cyan-400 hover:underline"
                  >
                    {s.deployment_name ?? s.deployment_id.slice(0, 8)} →
                  </Link>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-2 md:justify-end">
                {s.sdk_endpoint_id && (
                  <span className="text-xs text-emerald-400" title="Registered as an SDK endpoint">
                    registered
                  </span>
                )}
                {isOperator && (
                  <SampleAction
                    sample={s}
                    busy={isBusy}
                    onDeploy={() => onDeploy(s.language)}
                    onReuse={() => onReuse(s.language)}
                    onUpdate={() => onUpdate(s.language)}
                  />
                )}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

function SampleAction({
  sample,
  busy,
  onDeploy,
  onReuse,
  onUpdate,
}: {
  sample: SdkSampleStatus;
  busy: boolean;
  onDeploy: () => void;
  onReuse: () => void;
  onUpdate: () => void;
}) {
  const label = SAMPLE_ACTION_LABEL[sample.recommended_action];

  if (sample.recommended_action === 'wait') {
    return <span className="text-xs text-cyan-400">{label}</span>;
  }

  if (sample.recommended_action === 'reuse') {
    // Already registered: reuse is a no-op, so say so instead of offering a
    // button that does nothing.
    if (sample.sdk_endpoint_id) {
      return <span className="text-xs text-faint">in use</span>;
    }
    return (
      <Button size="xs" onClick={onReuse} loading={busy} loadingLabel="Registering…">
        {label}
      </Button>
    );
  }

  if (sample.recommended_action === 'update') {
    return (
      <>
        <Button size="xs" variant="primary" onClick={onUpdate} loading={busy} loadingLabel="Updating…">
          {label} → {sample.current_version}
        </Button>
        {!sample.sdk_endpoint_id && (
          <Button size="xs" onClick={onReuse}>Register as-is</Button>
        )}
      </>
    );
  }

  // create / redeploy — both mean "provision a server", which is the dialog.
  return (
    <Button size="xs" variant={sample.recommended_action === 'redeploy' ? 'danger' : 'secondary'} onClick={onDeploy}>
      {label}
    </Button>
  );
}
