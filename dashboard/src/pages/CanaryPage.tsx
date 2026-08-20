import { useCallback, useState } from 'react';
import { Link } from 'react-router';
import { api, errorMessage } from '../api/client';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
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

type CanaryHistoryItem = Awaited<ReturnType<typeof api.getCanaryHistory>>['items'][number];
type CanaryRunItem = Awaited<ReturnType<typeof api.getCanaryRuns>>['runs'][number];

// queued/in_progress → pulse; success green; failure red; everything else gray.
function outcomeBadge(status: string | null, conclusion: string | null) {
  if (status && status !== 'completed') {
    return <span className="text-yellow-300 animate-pulse">{status}</span>;
  }
  if (!conclusion) return <span className="text-gray-500">—</span>;
  const cls =
    conclusion === 'success'
      ? 'text-emerald-400'
      : conclusion === 'failure'
        ? 'text-red-400'
        : 'text-gray-400';
  return <span className={cls}>{conclusion}</span>;
}

function fmtWhen(iso: string | null) {
  if (!iso) return '—';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

// The non-default inputs, compactly ("matrix_flow, windows" etc.).
function fmtInputs(inputs: Record<string, string>) {
  const on = Object.entries(inputs)
    .filter(([, v]) => v === '1')
    .map(([k]) => k);
  return on.length ? on.join(', ') : 'none';
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
  const [history, setHistory] = useState<CanaryHistoryItem[] | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [ghRuns, setGhRuns] = useState<CanaryRunItem[] | null>(null);
  const [ghDetail, setGhDetail] = useState<string | null>(null);

  const loadStatus = useCallback(() => {
    api
      .getCanaryStatus()
      .then((s) => {
        setStatus(s);
        setStatusError(null);
      })
      .catch((e: unknown) => setStatusError(errorMessage(e)));
  }, []);

  // Durable history from OUR database — must render even when GitHub is down.
  const loadHistory = useCallback(() => {
    api
      .getCanaryHistory()
      .then((r) => {
        setHistory(r.items);
        setHistoryError(null);
      })
      .catch((e: unknown) => setHistoryError(errorMessage(e)));
  }, []);

  // Live GitHub runs — best-effort; the endpoint degrades to an empty list.
  const loadGhRuns = useCallback(() => {
    api
      .getCanaryRuns()
      .then((r) => {
        setGhRuns(r.runs);
        setGhDetail(r.detail ?? null);
      })
      .catch((e: unknown) => setGhDetail(errorMessage(e)));
  }, []);

  // One 30s tick for all three fetches (status folded in rather than a
  // second 60s loop — it's one cheap GET and one timer beats two). 30s
  // because a canary run's queued→in_progress→conclusion transitions are
  // minutes apart; anything faster just burns GitHub API quota. Silent by
  // construction: none of the loaders sets a loading flag, so poll ticks swap
  // rows in place — only the initial null state renders as empty. All calls
  // are issued synchronously (usePolling's request-source contract).
  usePolling(() => {
    loadStatus();
    loadHistory();
    loadGhRuns();
  }, 30_000);

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
        loadHistory();
        loadGhRuns();
      })
      .catch((e: unknown) => toast('error', errorMessage(e)))
      .finally(() => setDispatching(false));
  }, [inputs, gitRef, toast, status, loadHistory, loadGhRuns]);

  const actionsUrl = status?.actions_url;

  return (
    <div className="p-4 md:p-6 max-w-5xl">
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
              className="px-3 py-1.5 text-sm font-semibold rounded bg-cyan-600 hover:bg-cyan-500 text-[var(--bg-base)] disabled:opacity-50 disabled:cursor-not-allowed"
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

      {/* ── Dispatch history (durable, our DB) ──────────────────────── */}
      <section className="mt-6 border border-gray-700 rounded bg-gray-900/40">
        <div className="px-4 py-3 border-b border-gray-700 flex items-center justify-between">
          <h3 className="text-sm font-semibold text-gray-200">Dispatch history</h3>
          <button
            type="button"
            onClick={() => {
              loadStatus();
              loadHistory();
              loadGhRuns();
            }}
            className="text-xs text-cyan-400 hover:text-cyan-300"
          >
            Refresh
          </button>
        </div>
        <div className="p-4">
          <p className="text-xs text-gray-500 mb-3">
            In-product triggers recorded in this deployment's database — visible even when GitHub is
            unreachable. Run link/outcome are backfilled automatically once the run appears on GitHub.
          </p>
          {historyError && <div className="text-sm text-red-400">Could not load history: {historyError}</div>}
          {history && history.length === 0 && !historyError && (
            <div className="text-sm text-gray-500">No in-product dispatches recorded yet.</div>
          )}
          {history && history.length > 0 && (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="text-left text-xs text-gray-500 border-b border-gray-800">
                    <th className="py-1.5 pr-3 font-medium">Requested</th>
                    <th className="py-1.5 pr-3 font-medium">By</th>
                    <th className="py-1.5 pr-3 font-medium">Ref</th>
                    <th className="py-1.5 pr-3 font-medium">Inputs on</th>
                    <th className="py-1.5 pr-3 font-medium">Outcome</th>
                    <th className="py-1.5 font-medium">Run</th>
                  </tr>
                </thead>
                <tbody>
                  {history.map((h) => (
                    <tr key={h.id} className="border-b border-gray-800/60 text-gray-300">
                      <td className="py-1.5 pr-3 whitespace-nowrap font-mono text-xs">{fmtWhen(h.requested_at)}</td>
                      <td className="py-1.5 pr-3 text-xs">{h.requested_by ?? '—'}</td>
                      <td className="py-1.5 pr-3 font-mono text-xs">{h.git_ref}</td>
                      <td className="py-1.5 pr-3 text-xs text-gray-400">{fmtInputs(h.inputs)}</td>
                      <td className="py-1.5 pr-3 text-xs">{outcomeBadge(h.run_status, h.conclusion)}</td>
                      <td className="py-1.5 text-xs">
                        {h.run_url ? (
                          <a
                            href={h.run_url}
                            target="_blank"
                            rel="noopener noreferrer"
                            className="text-cyan-400 hover:text-cyan-300 underline"
                          >
                            #{h.run_id} ↗
                          </a>
                        ) : (
                          <span className="text-gray-500">{h.conclusion === 'unresolved' ? 'not found' : 'linking…'}</span>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </section>

      {/* ── Recent GitHub runs (live) ───────────────────────────────── */}
      <section className="mt-6 border border-gray-700 rounded bg-gray-900/40">
        <div className="px-4 py-3 border-b border-gray-700">
          <h3 className="text-sm font-semibold text-gray-200">Recent runs on GitHub</h3>
        </div>
        <div className="p-4">
          <p className="text-xs text-gray-500 mb-3">
            Live from the GitHub API — includes canary runs triggered outside the product (Actions UI,
            schedules, other deployments).
          </p>
          {ghDetail && <div className="text-sm text-gray-500 mb-2">{ghDetail}</div>}
          {ghRuns && ghRuns.length === 0 && !ghDetail && (
            <div className="text-sm text-gray-500">No runs returned.</div>
          )}
          {ghRuns && ghRuns.length > 0 && (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="text-left text-xs text-gray-500 border-b border-gray-800">
                    <th className="py-1.5 pr-3 font-medium">Run</th>
                    <th className="py-1.5 pr-3 font-medium">Started</th>
                    <th className="py-1.5 pr-3 font-medium">Trigger</th>
                    <th className="py-1.5 pr-3 font-medium">Actor</th>
                    <th className="py-1.5 pr-3 font-medium">Branch</th>
                    <th className="py-1.5 font-medium">Outcome</th>
                  </tr>
                </thead>
                <tbody>
                  {ghRuns.map((r) => (
                    <tr key={r.id} className="border-b border-gray-800/60 text-gray-300">
                      <td className="py-1.5 pr-3 text-xs">
                        {r.htmlUrl ? (
                          <a
                            href={r.htmlUrl}
                            target="_blank"
                            rel="noopener noreferrer"
                            className="text-cyan-400 hover:text-cyan-300 underline"
                          >
                            #{r.runNumber} ↗
                          </a>
                        ) : (
                          <span>#{r.runNumber}</span>
                        )}
                      </td>
                      <td className="py-1.5 pr-3 whitespace-nowrap font-mono text-xs">{fmtWhen(r.createdAt)}</td>
                      <td className="py-1.5 pr-3 text-xs text-gray-400">{r.event ?? '—'}</td>
                      <td className="py-1.5 pr-3 text-xs">{r.actor ?? '—'}</td>
                      <td className="py-1.5 pr-3 font-mono text-xs">{r.branch ?? '—'}</td>
                      <td className="py-1.5 text-xs">{outcomeBadge(r.status, r.conclusion)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

export default CanaryPage;
