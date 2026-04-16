import { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { api } from '../api/client';
import type { EndpointRef, TestConfigCreate, TestRun, Workload } from '../api/types';
import { Breadcrumb } from '../components/common/Breadcrumb';
import { StatusBadge } from '../components/common/StatusBadge';
import { usePageTitle } from '../hooks/usePageTitle';
import { usePolling } from '../hooks/usePolling';
import { useProject } from '../hooks/useProject';
import { useToast } from '../hooks/useToast';

type ProbePreset = 'quick' | 'standard' | 'full';

const PROBE_PRESETS: Record<ProbePreset, string[]> = {
  quick: ['dns', 'tcp', 'tls', 'http2'],
  standard: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp'],
  full: ['dns', 'tcp', 'tls', 'tlsresume', 'native', 'http1', 'http2', 'http3', 'udp', 'curl', 'pageload', 'pageload2', 'pageload3', 'browser1', 'browser2', 'browser3'],
};

const PROBE_PRESET_LABELS: Record<ProbePreset, { time: string; desc: string }> = {
  quick: { time: '~3s', desc: 'dns, tcp, tls, http2' },
  standard: { time: '~15s', desc: '+ http1, http3, tls-resume, native-tls, udp' },
  full: { time: '~60s', desc: '+ pageload, browser' },
};

function extractHost(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return '';
  try {
    if (trimmed.includes('://')) {
      return new URL(trimmed).hostname;
    }
    const candidate = new URL(`https://${trimmed}`);
    return candidate.hostname;
  } catch {
    return trimmed;
  }
}

/** Parse host from probe config_name format: "Probe: hostname (Preset)" */
function parseProbeHost(configName: string | undefined): string | null {
  if (!configName) return null;
  const match = configName.match(/^Probe:\s+(.+?)\s+\(/);
  return match ? match[1] : null;
}

/** Group runs by day label */
function getDayLabel(dateStr: string): string {
  const date = new Date(dateStr);
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const runDay = new Date(date.getFullYear(), date.getMonth(), date.getDate());

  if (runDay.getTime() === today.getTime()) return 'Today';
  if (runDay.getTime() === yesterday.getTime()) return 'Yesterday';
  return date.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' });
}

/** Format ms as readable duration */
function fmtMs(ms: number | null | undefined): string {
  if (ms == null) return '-';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

/** Format time from ISO date string */
function fmtTime(dateStr: string): string {
  return new Date(dateStr).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

export function ProbePage() {
  const { projectId } = useProject();
  const addToast = useToast();
  const [searchParams, setSearchParams] = useSearchParams();
  usePageTitle('Quick Probe');

  const inputRef = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState(searchParams.get('host') || '');
  const [preset, setPreset] = useState<ProbePreset>('quick');
  const [submitting, setSubmitting] = useState(false);
  const [allRuns, setAllRuns] = useState<TestRun[]>([]);
  const [loadingRuns, setLoadingRuns] = useState(true);
  const [pendingRunIds, setPendingRunIds] = useState<Set<string>>(new Set());

  // Sync URL input to query string
  useEffect(() => {
    const host = extractHost(url);
    setSearchParams(prev => {
      const next = new URLSearchParams(prev);
      if (host) {
        next.set('host', host);
      } else {
        next.delete('host');
      }
      return next;
    }, { replace: true });
  }, [url, setSearchParams]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Fetch all probe runs
  const loadProbeRuns = useCallback(() => {
    if (!projectId) return;
    api
      .listTestRuns(projectId, { endpoint_kind: 'network', limit: 50 })
      .then((data) => {
        // Filter to only probe-style runs (config_name starts with "Probe:")
        const probeRuns = data.filter(r => r.config_name?.startsWith('Probe:'));
        setAllRuns(probeRuns);
        setLoadingRuns(false);

        // Clear completed pending runs
        setPendingRunIds(prev => {
          const stillPending = new Set<string>();
          for (const id of prev) {
            const run = probeRuns.find(r => r.id === id);
            if (run && (run.status === 'queued' || run.status === 'running')) {
              stillPending.add(id);
            }
          }
          return stillPending.size !== prev.size ? stillPending : prev;
        });
      })
      .catch(() => {
        setLoadingRuns(false);
      });
  }, [projectId]);

  // Poll faster when there are pending runs
  const hasPending = pendingRunIds.size > 0;
  usePolling(loadProbeRuns, hasPending ? 5000 : 15000);

  // Extract unique hosts from probe runs for "recent hosts" chips
  const recentHosts = useMemo(() => {
    const hostSet = new Map<string, number>();
    for (const run of allRuns) {
      const host = parseProbeHost(run.config_name);
      if (host && !hostSet.has(host)) {
        hostSet.set(host, new Date(run.created_at).getTime());
      }
    }
    return Array.from(hostSet.entries())
      .sort((a, b) => b[1] - a[1])
      .map(([host]) => host)
      .slice(0, 8);
  }, [allRuns]);

  // Filter runs by current host
  const currentHost = extractHost(url);
  const filteredRuns = useMemo(() => {
    if (!currentHost) return [];
    return allRuns.filter(r => {
      const host = parseProbeHost(r.config_name);
      return host === currentHost;
    });
  }, [allRuns, currentHost]);

  // Group filtered runs by day
  const groupedRuns = useMemo(() => {
    const groups: Array<{ label: string; runs: TestRun[] }> = [];
    let currentLabel = '';
    for (const run of filteredRuns) {
      const label = getDayLabel(run.created_at);
      if (label !== currentLabel) {
        groups.push({ label, runs: [run] });
        currentLabel = label;
      } else {
        groups[groups.length - 1].runs.push(run);
      }
    }
    return groups;
  }, [filteredRuns]);

  const handleProbe = async () => {
    const host = extractHost(url);
    if (!host) {
      addToast('error', 'Enter a URL or hostname to probe');
      return;
    }

    setSubmitting(true);
    try {
      const presetLabel = preset.charAt(0).toUpperCase() + preset.slice(1);
      const configName = `Probe: ${host} (${presetLabel})`;

      const endpoint: EndpointRef = { kind: 'network', host };

      const workload: Workload = {
        modes: PROBE_PRESETS[preset],
        runs: 1,
        concurrency: 1,
        timeout_ms: 5000,
        payload_sizes: [],
        capture_mode: 'headers-only',
      };

      const config: TestConfigCreate = {
        name: configName,
        endpoint,
        workload,
      };

      const created = await api.createTestConfig(projectId, config);
      const run = await api.launchTestConfig(created.id);
      addToast('success', `Probe ${run.id.slice(0, 8)} launched`);

      // Stay on page - track the pending run
      setPendingRunIds(prev => new Set(prev).add(run.id));

      // Immediately add to local state so it appears in the list
      setAllRuns(prev => [run, ...prev]);
    } catch (e) {
      addToast('error', `Probe failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setSubmitting(false);
    }
  };

  const handleHostClick = (host: string) => {
    setUrl(host);
    inputRef.current?.focus();
  };

  return (
    <div className="p-4 md:p-6 flex flex-col items-center">
      <div className="w-full max-w-2xl">
        <Breadcrumb items={[{ label: 'Runs', to: `/projects/${projectId}/runs` }, { label: 'Quick Probe' }]} />

        {/* Probe form section */}
        <div className="mt-8 mb-6">
          <h2 className="text-xl font-bold text-gray-100 mb-1">Quick Probe</h2>
          <p className="text-xs text-gray-500">Test any URL in seconds. Results appear below.</p>
        </div>

        {/* URL input + run button */}
        <div className="flex gap-2 mb-4">
          <div className="flex-1">
            <label htmlFor="probe-url" className="sr-only">URL or hostname</label>
            <input
              ref={inputRef}
              id="probe-url"
              type="text"
              value={url}
              onChange={e => setUrl(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter' && url.trim()) handleProbe(); }}
              placeholder="e.g. www.cloudflare.com"
              className="bg-[var(--bg-base)] border border-gray-700 rounded px-4 py-3 text-sm font-mono text-gray-200 w-full focus:outline-none focus:border-cyan-500 placeholder:text-gray-600"
              aria-label="URL or hostname to probe"
            />
          </div>
          <button
            onClick={handleProbe}
            disabled={submitting || !url.trim()}
            className="bg-cyan-600 hover:bg-cyan-500 disabled:opacity-40 disabled:cursor-not-allowed text-white px-5 py-3 rounded text-sm font-medium transition-colors whitespace-nowrap"
            aria-label="Run probe"
          >
            {submitting ? 'Launching...' : 'Run Probe'}
          </button>
        </div>

        {/* Preset toggle buttons */}
        <div className="mb-4">
          <div className="flex gap-2">
            {(['quick', 'standard', 'full'] as ProbePreset[]).map(p => (
              <button
                key={p}
                onClick={() => setPreset(p)}
                className={`border rounded px-3 py-1.5 text-xs transition-colors ${
                  preset === p
                    ? 'border-cyan-500 bg-cyan-500/10 text-cyan-400'
                    : 'border-gray-800 text-gray-500 hover:border-gray-600 hover:text-gray-400'
                }`}
                aria-pressed={preset === p}
              >
                {p.charAt(0).toUpperCase() + p.slice(1)}
                <span className="ml-1.5 text-gray-600">{PROBE_PRESET_LABELS[p].time}</span>
              </button>
            ))}
          </div>
        </div>

        {/* Recent host chips */}
        {recentHosts.length > 0 && (
          <div className="mb-6 flex flex-wrap gap-1.5 items-center">
            <span className="text-xs text-gray-600 mr-1">Recent:</span>
            {recentHosts.map(host => (
              <button
                key={host}
                onClick={() => handleHostClick(host)}
                className={`text-xs px-2 py-1 rounded border transition-colors font-mono ${
                  currentHost === host
                    ? 'border-cyan-500/40 bg-cyan-500/10 text-cyan-400'
                    : 'border-gray-800 text-gray-500 hover:border-gray-600 hover:text-gray-400'
                }`}
              >
                {host.length > 30 ? host.slice(0, 27) + '...' : host}
              </button>
            ))}
          </div>
        )}

        {/* Divider */}
        <div className="border-t border-gray-800 my-6" />

        {/* Probe history section */}
        <div className="mb-6">
          <h3 className="text-sm font-semibold text-gray-300 mb-4">
            {currentHost ? (
              <>Probe History — <span className="font-mono text-cyan-400">{currentHost}</span></>
            ) : (
              'Probe History'
            )}
          </h3>

          {!currentHost ? (
            <div className="border border-gray-800 rounded p-8 text-center">
              <p className="text-gray-500 text-sm">Enter a URL above to see probe history</p>
            </div>
          ) : loadingRuns ? (
            <div className="space-y-2">
              {[1, 2, 3].map(i => (
                <div key={i} className="border border-gray-800/50 rounded p-3 flex gap-4">
                  <div className="h-3 w-12 bg-gray-800 rounded motion-safe:animate-pulse" />
                  <div className="h-3 w-20 bg-gray-800/60 rounded motion-safe:animate-pulse" />
                  <div className="flex-1" />
                  <div className="h-3 w-16 bg-gray-800/40 rounded motion-safe:animate-pulse" />
                </div>
              ))}
            </div>
          ) : filteredRuns.length === 0 ? (
            <div className="border border-gray-800 rounded p-8 text-center">
              <p className="text-gray-500 text-sm">No probes for this host yet. Run your first probe above.</p>
            </div>
          ) : (
            <div className="space-y-4">
              {groupedRuns.map(group => (
                <div key={group.label}>
                  <div className="text-xs text-gray-600 mb-2 font-medium">{group.label}</div>
                  <div className="space-y-1">
                    {group.runs.map(run => (
                      <ProbeHistoryRow
                        key={run.id}
                        run={run}
                        projectId={projectId}
                        isPending={pendingRunIds.has(run.id)}
                      />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Link to configured run */}
        <div className="mt-8 pt-6 border-t border-gray-800 text-center">
          <p className="text-xs text-gray-600">
            Need a full configured run?{' '}
            <Link
              to={`/projects/${projectId}/runs/new`}
              className="text-cyan-400 hover:text-cyan-300 transition-colors"
            >
              Go to New Run
            </Link>
          </p>
        </div>
      </div>
    </div>
  );
}

function ProbeHistoryRow({ run, projectId, isPending }: { run: TestRun; projectId: string; isPending: boolean }) {
  const isActive = run.status === 'queued' || run.status === 'running';
  const presetMatch = run.config_name?.match(/\((\w+)\)$/);
  const presetLabel = presetMatch ? presetMatch[1] : null;

  // Compute a rough duration from started_at to finished_at
  const durationMs = run.started_at && run.finished_at
    ? new Date(run.finished_at).getTime() - new Date(run.started_at).getTime()
    : null;

  return (
    <Link
      to={`/projects/${projectId}/runs/${run.id}`}
      className={`flex items-center gap-3 px-3 py-2.5 rounded border transition-colors ${
        isPending && isActive
          ? 'border-cyan-500/30 bg-cyan-500/5'
          : 'border-gray-800/50 hover:border-gray-700 hover:bg-gray-800/20'
      }`}
    >
      {/* Time */}
      <span className="text-xs text-gray-500 font-mono w-12 flex-shrink-0">
        {fmtTime(run.created_at)}
      </span>

      {/* Status */}
      <span className="flex-shrink-0">
        <StatusBadge status={run.status} />
      </span>

      {/* Preset tag */}
      {presetLabel && (
        <span className="text-[10px] text-gray-600 bg-gray-800/60 px-1.5 py-0.5 rounded flex-shrink-0">
          {presetLabel}
        </span>
      )}

      {/* Spacer */}
      <span className="flex-1" />

      {/* Results summary */}
      {run.status === 'completed' && (
        <span className="flex items-center gap-3 text-xs font-mono">
          <span className="text-green-400">{run.success_count} ok</span>
          {run.failure_count > 0 && (
            <span className="text-red-400">{run.failure_count} fail</span>
          )}
        </span>
      )}

      {/* Duration */}
      {durationMs != null && (
        <span className="text-xs text-gray-600 font-mono w-14 text-right flex-shrink-0">
          {fmtMs(durationMs)}
        </span>
      )}

      {/* Pending indicator */}
      {isPending && isActive && (
        <span className="w-1.5 h-1.5 rounded-full bg-cyan-400 motion-safe:animate-pulse flex-shrink-0" />
      )}
    </Link>
  );
}
