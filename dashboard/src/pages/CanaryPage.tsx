import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, errorMessage } from '../api/client';
import { usePageTitle } from '../hooks/usePageTitle';
import { useToast } from '../hooks/useToast';
import { useProjectStore } from '../stores/projectStore';

// Admin-only surface for the prod run-execution canary
// (.github/workflows/soak-canary.yml). Two capabilities:
//   1. "Run canary" — dispatches the workflow via the control plane, which
//      calls the GitHub REST workflow_dispatch API with a server-side token.
//   2. "Logs in a new tab" — links out to the GitHub Actions run log and to
//      the in-product runs/deployments the canary actually drives.

interface CanaryStatus {
  configured: boolean;
  owner: string;
  repo: string;
  workflow: string;
  actions_url: string;
}

// Mirrors soak-canary.yml's workflow_dispatch inputs + their defaults.
interface CanaryInputs {
  reuse_runner: boolean;
  apibench: boolean;
  mode_coverage: boolean;
  matrix_flow: boolean;
  windows: boolean;
}

const DEFAULT_INPUTS: CanaryInputs = {
  reuse_runner: true,
  apibench: true,
  mode_coverage: true,
  matrix_flow: false,
  windows: false,
};

const INPUT_META: { key: keyof CanaryInputs; label: string; help: string }[] = [
  { key: 'reuse_runner', label: 'Reuse runner', help: 'Reuse an idle runner instead of provisioning a fresh one.' },
  { key: 'apibench', label: 'apibench phase', help: 'Also run the proxy-fronted apibench phase.' },
  { key: 'mode_coverage', label: 'Mode coverage', help: 'Also run the full HTTP/TCP mode matrix through the proxy.' },
  { key: 'matrix_flow', label: 'Matrix flow', help: 'Multi-cell matrix flow — provisions ~3 VMs.' },
  { key: 'windows', label: 'Windows/IIS cell', help: 'Also provision one Windows Server + IIS endpoint cell.' },
];

export function CanaryPage() {
  usePageTitle('Canary');
  const toast = useToast();
  const activeProjectId = useProjectStore((s) => s.activeProjectId);

  const [status, setStatus] = useState<CanaryStatus | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [inputs, setInputs] = useState<CanaryInputs>(DEFAULT_INPUTS);
  const [gitRef, setGitRef] = useState('main');
  const [dispatching, setDispatching] = useState(false);

  const loadStatus = useCallback(() => {
    api
      .getCanaryStatus()
      .then((s) => {
        setStatus(s);
        setStatusError(null);
      })
      .catch((e: unknown) => setStatusError(errorMessage(e)));
  }, []);

  useEffect(loadStatus, [loadStatus]);

  const dispatch = useCallback(() => {
    // Short-circuit the common local case with a clean message — the endpoint
    // still returns its own 409 "not configured" (for API callers), but we
    // avoid surfacing the generic HTTP-409 status copy in the UI.
    if (status && !status.configured) {
      toast(
        'error',
        'Canary dispatch is not configured. Set CANARY_GITHUB_TOKEN on the control plane to enable it.',
      );
      return;
    }
    setDispatching(true);
    api
      .dispatchCanary({ ...inputs, ref: gitRef.trim() || 'main' })
      .then((r) => {
        toast('success', 'Canary dispatched. Opening the GitHub Actions log…');
        window.open(r.actions_url, '_blank', 'noopener,noreferrer');
      })
      .catch((e: unknown) => toast('error', errorMessage(e)))
      .finally(() => setDispatching(false));
  }, [inputs, gitRef, toast, status]);

  const actionsUrl = status?.actions_url;

  return (
    <div className="p-4 md:p-6 max-w-3xl">
      <h2 className="text-lg md:text-xl font-bold text-gray-100 mb-1">Run-execution canary</h2>
      <p className="text-sm text-gray-400 mb-6">
        Trigger the prod run-execution canary (<code className="text-gray-300">soak-canary.yml</code>) and
        jump to its results. It drives a real run end-to-end against prod and asserts the pipeline is healthy.
      </p>

      {statusError && (
        <div className="mb-4 border border-red-500/40 bg-red-500/10 text-red-300 text-sm px-3 py-2 rounded">
          Could not load canary status: {statusError}
        </div>
      )}

      {status && !status.configured && (
        <div className="mb-4 border border-yellow-500/40 bg-yellow-500/10 text-yellow-200 text-sm px-3 py-2 rounded">
          Dispatch is <span className="font-semibold">not configured</span> on this control plane. Set{' '}
          <code className="text-yellow-100">CANARY_GITHUB_TOKEN</code> (a token with{' '}
          <code className="text-yellow-100">actions:write</code> on{' '}
          <code className="text-yellow-100">{status.owner}/{status.repo}</code>) to enable the button. You can
          still open the Actions log below.
        </div>
      )}

      {/* ── Trigger ─────────────────────────────────────────────────── */}
      <section className="mb-6 border border-gray-700 rounded bg-gray-900/40">
        <div className="px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-gray-200">Trigger</h3>
        </div>
        <div className="p-4 space-y-3">
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            {INPUT_META.map((m) => (
              <label
                key={m.key}
                className="flex items-start gap-2 text-sm text-gray-300 cursor-pointer"
                title={m.help}
              >
                <input
                  type="checkbox"
                  className="mt-0.5 accent-cyan-500"
                  checked={inputs[m.key]}
                  onChange={(e) => setInputs((prev) => ({ ...prev, [m.key]: e.target.checked }))}
                />
                <span>
                  <span className="font-medium">{m.label}</span>
                  <span className="block text-xs text-gray-500">{m.help}</span>
                </span>
              </label>
            ))}
          </div>

          <div className="flex items-center gap-2 pt-1">
            <label className="text-sm text-gray-400" htmlFor="canary-ref">
              Ref
            </label>
            <input
              id="canary-ref"
              type="text"
              value={gitRef}
              onChange={(e) => setGitRef(e.target.value)}
              className="bg-gray-800 border border-gray-700 rounded px-2 py-1 text-sm text-gray-200 font-mono w-40"
              placeholder="main"
            />
          </div>

          <div className="pt-2">
            <button
              type="button"
              onClick={dispatch}
              disabled={dispatching}
              className="px-3 py-1.5 text-sm font-semibold rounded bg-cyan-600 hover:bg-cyan-500 text-white disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {dispatching ? 'Dispatching…' : 'Run canary'}
            </button>
            {status && !status.configured && (
              <span className="ml-3 text-xs text-gray-500">
                Button works, but returns a clear "not configured" error until a token is set.
              </span>
            )}
          </div>
        </div>
      </section>

      {/* ── Results / logs ──────────────────────────────────────────── */}
      <section className="border border-gray-700 rounded bg-gray-900/40">
        <div className="px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-gray-200">Results &amp; logs</h3>
        </div>
        <ul className="p-4 space-y-2 text-sm">
          <li>
            {actionsUrl ? (
              <a
                href={actionsUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="text-cyan-400 hover:text-cyan-300 underline"
              >
                GitHub Actions run log ↗
              </a>
            ) : (
              <span className="text-gray-500">GitHub Actions run log</span>
            )}
            <span className="block text-xs text-gray-500">
              Opens the workflow's run history on GitHub in a new tab.
            </span>
          </li>
          <li>
            {activeProjectId ? (
              <Link
                to={`/projects/${activeProjectId}/runs?q=soak-canary`}
                target="_blank"
                rel="noopener noreferrer"
                className="text-cyan-400 hover:text-cyan-300 underline"
              >
                Canary runs in this project ↗
              </Link>
            ) : (
              <span className="text-gray-500">Canary runs (select a project first)</span>
            )}
            <span className="block text-xs text-gray-500">
              The canary's runs/deployments show up in the product; its configs are named{' '}
              <code className="text-gray-400">soak-canary-*</code>. Opens the Runs list filtered to them.
            </span>
          </li>
        </ul>
      </section>
    </div>
  );
}

export default CanaryPage;
