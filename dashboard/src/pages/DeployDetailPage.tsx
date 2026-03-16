import { useState, useCallback, useEffect, useRef } from 'react';
import { useParams, Link, useNavigate } from 'react-router-dom';
import { api } from '../api/client';
import type { Deployment } from '../api/types';
import { usePolling } from '../hooks/usePolling';
import { useLiveStore } from '../stores/liveStore';
import { useToast } from '../hooks/useToast';

const STATUS_COLORS: Record<string, string> = {
  pending: 'text-gray-400',
  running: 'text-cyan-400',
  completed: 'text-green-400',
  failed: 'text-red-400',
  cancelled: 'text-yellow-400',
};

const STATUS_BG: Record<string, string> = {
  pending: 'bg-gray-500',
  running: 'bg-cyan-500 motion-safe:animate-pulse',
  completed: 'bg-green-500',
  failed: 'bg-red-500',
  cancelled: 'bg-yellow-500',
};

interface EndpointHealth {
  ip: string;
  alive: boolean;
  version?: string;
  outdated?: boolean;
}

const EMPTY_LINES: string[] = [];

export function DeployDetailPage() {
  const { deploymentId } = useParams<{ deploymentId: string }>();
  const [deployment, setDeployment] = useState<Deployment | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [endpointHealth, setEndpointHealth] = useState<EndpointHealth[]>([]);
  const [healthLoading, setHealthLoading] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [versionInfo, setVersionInfo] = useState<{ latest: string | null; endpointVersion: string | null } | null>(null);
  const logContainerRef = useRef<HTMLDivElement>(null);
  const [autoScroll, setAutoScroll] = useState(true);
  const addToast = useToast();
  const navigate = useNavigate();

  // Live log lines from WebSocket — must return a stable reference to avoid infinite re-renders
  const liveLines = useLiveStore(s => (deploymentId && s.deployLogs[deploymentId]) || EMPTY_LINES);

  const loadDeployment = useCallback(async () => {
    if (!deploymentId) return;
    try {
      const data = await api.getDeployment(deploymentId);
      setDeployment(data);
      setError(null);
    } catch {
      setError('Failed to load deployment');
    } finally {
      setLoading(false);
    }
  }, [deploymentId]);

  usePolling(loadDeployment, 5000, !!deploymentId);

  // Auto-scroll log container when new lines arrive
  useEffect(() => {
    if (autoScroll && logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [liveLines, autoScroll]);

  const handleLogScroll = () => {
    if (!logContainerRef.current) return;
    const { scrollTop, scrollHeight, clientHeight } = logContainerRef.current;
    setAutoScroll(scrollHeight - scrollTop - clientHeight < 50);
  };

  const handleStop = async () => {
    if (!deploymentId) return;
    try {
      await api.stopDeployment(deploymentId);
      addToast('info', 'Deployment stop requested');
      loadDeployment();
    } catch {
      addToast('error', 'Failed to stop deployment');
    }
  };

  const handleCheckHealth = async () => {
    if (!deploymentId) return;
    setHealthLoading(true);
    try {
      const result = await api.checkDeployment(deploymentId) as { endpoints: EndpointHealth[]; latest_release?: string };
      setEndpointHealth(result.endpoints);
      if (result.latest_release) {
        const epVer = result.endpoints.find(e => e.version)?.version ?? null;
        setVersionInfo({ latest: result.latest_release, endpointVersion: epVer });
      }
    } catch {
      addToast('error', 'Failed to check endpoint health');
    } finally {
      setHealthLoading(false);
    }
  };

  const handleDelete = async () => {
    if (!deploymentId) return;
    try {
      await api.deleteDeployment(deploymentId);
      addToast('success', 'Deployment deleted');
      navigate('/deploy');
    } catch {
      addToast('error', 'Failed to delete deployment');
    }
  };

  // Use live WebSocket lines while deployment is active, DB log when done.
  const logLines: string[] =
    liveLines.length > 0
      ? liveLines
      : deployment?.log
        ? deployment.log.split('\n').filter(l => l.length > 0)
        : [];

  if (loading && !deployment) {
    return (
      <div className="p-6">
        <p className="text-gray-500">Loading deployment...</p>
      </div>
    );
  }

  if (error && !deployment) {
    return (
      <div className="p-6">
        <div className="bg-red-500/10 border border-red-500/30 rounded p-3">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      </div>
    );
  }

  const isActive = deployment?.status === 'running' || deployment?.status === 'pending';
  const isDone = deployment?.status === 'completed' || deployment?.status === 'failed' || deployment?.status === 'cancelled';
  const hasEndpoints = deployment?.endpoint_ips && Array.isArray(deployment.endpoint_ips) && deployment.endpoint_ips.length > 0;

  return (
    <div className="p-6">
      {/* Breadcrumb */}
      <div className="flex items-center gap-2 text-sm text-gray-500 mb-4">
        <Link to="/deploy" className="hover:text-gray-300">
          Deployments
        </Link>
        <span>/</span>
        <span className="text-gray-300">{deployment?.name || deploymentId?.slice(0, 8)}</span>
      </div>

      {/* Header */}
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-3">
          <span className={`w-3 h-3 rounded-full ${STATUS_BG[deployment?.status || 'pending']}`} />
          <h2 className="text-xl font-bold text-gray-100">{deployment?.name}</h2>
          <span className={`text-sm ${STATUS_COLORS[deployment?.status || 'pending']}`}>
            {deployment?.status}
          </span>
        </div>
        <div className="flex gap-2">
          {isActive && (
            <button
              onClick={handleStop}
              className="bg-red-600/20 border border-red-500/30 hover:bg-red-600/30 text-red-400 px-4 py-1.5 rounded text-sm transition-colors"
            >
              Stop
            </button>
          )}
          {isDone && (
            <>
              {hasEndpoints && (
                <button
                  onClick={handleCheckHealth}
                  disabled={healthLoading}
                  className="bg-[#12131a] border border-gray-700 hover:border-cyan-500 text-gray-300 hover:text-cyan-400 px-4 py-1.5 rounded text-sm transition-colors disabled:opacity-50"
                >
                  {healthLoading ? 'Checking...' : 'Check Health'}
                </button>
              )}
              {endpointHealth.some(ep => ep.outdated) && (
                <button
                  onClick={async () => {
                    if (!deploymentId) return;
                    try {
                      await api.updateEndpoint(deploymentId);
                      addToast('info', 'Endpoint update started');
                      loadDeployment();
                    } catch {
                      addToast('error', 'Failed to start update');
                    }
                  }}
                  className="bg-yellow-600/20 border border-yellow-500/30 hover:bg-yellow-600/30 text-yellow-400 px-4 py-1.5 rounded text-sm transition-colors"
                >
                  Update Endpoint
                </button>
              )}
              {!confirmDelete ? (
                <button
                  onClick={() => setConfirmDelete(true)}
                  className="bg-red-600/20 border border-red-500/30 hover:bg-red-600/30 text-red-400 px-4 py-1.5 rounded text-sm transition-colors"
                >
                  Delete
                </button>
              ) : (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-red-400">Delete this deployment?</span>
                  <button
                    onClick={handleDelete}
                    className="bg-red-600 hover:bg-red-500 text-white px-3 py-1.5 rounded text-sm transition-colors"
                  >
                    Confirm
                  </button>
                  <button
                    onClick={() => setConfirmDelete(false)}
                    className="text-gray-400 hover:text-gray-200 px-3 py-1.5 text-sm"
                  >
                    Cancel
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      {/* Info Cards */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 mb-1">Provider</p>
          <p className="text-sm text-gray-200">{deployment?.provider_summary || '\u2014'}</p>
        </div>
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 mb-1">Started</p>
          <p className="text-sm text-gray-200">
            {deployment?.started_at
              ? new Date(deployment.started_at).toLocaleString()
              : '\u2014'}
          </p>
        </div>
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 mb-1">Endpoint IPs</p>
          <p className="text-sm text-gray-200">
            {hasEndpoints
              ? (deployment?.endpoint_ips || []).join(', ')
              : '\u2014'}
          </p>
        </div>
        <div className="bg-[#12131a] border border-gray-800 rounded-lg p-4">
          <p className="text-xs text-gray-500 mb-1">Duration</p>
          <p className="text-sm text-gray-200">
            {deployment?.started_at
              ? formatDuration(deployment.started_at, deployment.finished_at)
              : '\u2014'}
          </p>
        </div>
      </div>

      {/* Endpoint Health */}
      {endpointHealth.length > 0 && (
        <div className="mb-6">
          <p className="text-sm text-gray-400 font-medium mb-2">Endpoint Health</p>
          <div className="flex flex-wrap gap-3">
            {endpointHealth.map(ep => (
              <div
                key={ep.ip}
                className={`bg-[#12131a] border rounded-lg p-3 flex items-center gap-3 ${
                  ep.alive ? (ep.outdated ? 'border-yellow-500/30' : 'border-green-500/30') : 'border-red-500/30'
                }`}
              >
                <span className={`w-2 h-2 rounded-full ${ep.alive ? (ep.outdated ? 'bg-yellow-400' : 'bg-green-400') : 'bg-red-400'}`} />
                <div>
                  <span className="text-sm text-gray-200 font-mono">{ep.ip.split('.')[0]}</span>
                  <div className="flex items-center gap-2 mt-0.5">
                    {ep.alive ? (
                      <>
                        <span className={`text-xs font-mono ${ep.outdated ? 'text-yellow-400' : 'text-green-400'}`}>
                          v{ep.version || '?'}
                        </span>
                        {ep.outdated && versionInfo?.latest && (
                          <span className="text-xs text-gray-600">
                            (latest: v{versionInfo.latest})
                          </span>
                        )}
                      </>
                    ) : (
                      <span className="text-xs text-red-400">unreachable</span>
                    )}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Error message */}
      {deployment?.error_message && (
        <div className="bg-red-500/10 border border-red-500/30 rounded p-3 mb-4">
          <p className="text-red-400 text-sm">{deployment.error_message}</p>
        </div>
      )}

      {/* Log Output */}
      <div className="mb-6">
        <div className="flex items-center justify-between mb-2">
          <p className="text-sm text-gray-400 font-medium">Deployment Log</p>
          {!autoScroll && (
            <button
              onClick={() => {
                setAutoScroll(true);
                if (logContainerRef.current) {
                  logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
                }
              }}
              className="text-xs text-cyan-400 hover:text-cyan-300"
            >
              Scroll to bottom
            </button>
          )}
        </div>
        <div
          ref={logContainerRef}
          onScroll={handleLogScroll}
          className="bg-[#0a0b0f] border border-gray-800 rounded-lg p-4 h-[400px] overflow-y-auto font-mono text-xs leading-5"
        >
          {logLines.length === 0 ? (
            <p className="text-gray-600">
              {isActive ? 'Waiting for output...' : 'No log output'}
            </p>
          ) : (
            logLines.map((line, i) => (
              <div key={i} className="text-gray-300 whitespace-pre-wrap break-all">
                {line}
              </div>
            ))
          )}
        </div>
      </div>

      {/* Config */}
      <details className="mb-6">
        <summary className="text-sm text-gray-400 cursor-pointer hover:text-gray-300 mb-2">
          Deployment Config
        </summary>
        <pre className="bg-[#0a0b0f] border border-gray-800 rounded-lg p-4 text-xs text-gray-400 overflow-x-auto">
          {JSON.stringify(deployment?.config, null, 2)}
        </pre>
      </details>
    </div>
  );
}

function formatDuration(start: string, end: string | null): string {
  const s = new Date(start).getTime();
  const e = end ? new Date(end).getTime() : Date.now();
  const secs = Math.round((e - s) / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${secs % 60}s`;
  const hours = Math.floor(mins / 60);
  return `${hours}h ${mins % 60}m`;
}
