import type { SdkEndpointCreate } from '../api/types';
import { buttonClassName } from './common/button-styles';
import { Button } from './common/Button';
import { endpointDraftForExample, SDK_EXAMPLES, type SdkExample } from '../lib/sdkExamples';

interface SdkExamplesPanelProps {
  isOperator: boolean;
  onUseExample: (draft: Partial<SdkEndpointCreate>) => void;
  examples?: readonly SdkExample[];
}

const AZURE_GUIDE_URL =
  'https://github.com/irlm/networker-tester/tree/main/examples/azure';

export function SdkExamplesPanel({
  isOperator,
  onUseExample,
  examples = SDK_EXAMPLES,
}: SdkExamplesPanelProps) {
  return (
    <section className="mb-6 overflow-hidden rounded-lg border border-gray-800" aria-labelledby="sdk-examples-title">
      <div className="flex flex-col gap-3 border-b border-gray-800 bg-[var(--bg-surface)] p-4 md:flex-row md:items-start md:justify-between">
        <div className="max-w-2xl">
          <h2 id="sdk-examples-title" className="text-sm font-bold text-gray-100">
            From working app to first signal
          </h2>
          <p className="mt-1 text-xs text-gray-400">
            Open a real SDK host, then register it here to see the same network-versus-application split your service will report.
          </p>
        </div>
        <a
          href={AZURE_GUIDE_URL}
          target="_blank"
          rel="noreferrer"
          className="text-xs text-cyan-400 hover:underline"
        >
          Azure deployment guide ↗
        </a>
      </div>

      <div className="divide-y divide-gray-800">
        {examples.map((example) => {
          const draft = endpointDraftForExample(example);
          return (
            <article
              key={example.id}
              className="grid gap-4 p-4 md:grid-cols-[minmax(0,1fr)_minmax(15rem,0.8fr)_auto] md:items-center"
            >
              <div>
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <h3 className="text-sm font-semibold text-gray-200">{example.language}</h3>
                  <span className="text-xs text-faint">{example.runtime}</span>
                </div>
                <p className="mt-1 text-xs text-gray-400">{example.description}</p>
              </div>

              <div className="min-w-0">
                <div className="flex items-center gap-2 text-xs">
                  <span
                    className={`h-1.5 w-1.5 rounded-full ${example.liveUrl ? 'bg-emerald-400' : 'bg-gray-600'}`}
                    aria-hidden="true"
                  />
                  <span className={example.liveUrl ? 'text-emerald-400' : 'text-gray-400'}>
                    {example.liveUrl ? 'Live on Azure' : 'Deploy ready'}
                  </span>
                </div>
                <div className="mt-1 break-words text-xs text-faint md:truncate" title={example.liveUrl}>
                  {example.liveUrl ? new URL(example.liveUrl).host : 'Add the deployment URL to enable one-click registration'}
                </div>
              </div>

              <div className="flex flex-wrap items-center gap-2 md:justify-end">
                {example.liveUrl && (
                  <a
                    href={example.liveUrl}
                    target="_blank"
                    rel="noreferrer"
                    className={buttonClassName({ variant: 'secondary', size: 'xs' })}
                  >
                    Open live app
                  </a>
                )}
                <a
                  href={example.sourceUrl}
                  target="_blank"
                  rel="noreferrer"
                  className={buttonClassName({ variant: 'ghost', size: 'xs' })}
                >
                  Source
                </a>
                {isOperator && draft && (
                  <Button size="xs" onClick={() => onUseExample(draft)}>
                    Use example
                  </Button>
                )}
              </div>
            </article>
          );
        })}
      </div>

      <ol className="grid border-t border-gray-800 bg-[var(--bg-surface)] text-xs text-gray-400 sm:grid-cols-4">
        {['Mount the SDK', 'Deploy the app', 'Register its URL', 'Run sdkprobe'].map((step, index) => (
          <li key={step} className="flex items-center gap-2 border-b border-gray-800 px-4 py-3 last:border-b-0 sm:border-b-0 sm:border-r sm:last:border-r-0">
            <span className="text-cyan-400" aria-hidden="true">{index + 1}</span>
            {step}
          </li>
        ))}
      </ol>
    </section>
  );
}
