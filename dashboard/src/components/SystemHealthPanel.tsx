import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

interface HealthCheck {
  check_name: string;
  status: string;
  value: string | null;
  message: string | null;
  checked_at: string;
}

interface HealthData {
  live: { core_db: boolean; logs_db: boolean };
  checks: HealthCheck[];
}

const STATUS_COLORS: Record<string, string> = {
  green: "text-emerald-400",
  yellow: "text-yellow-400",
  red: "text-red-400",
};

const STATUS_DOT: Record<string, string> = {
  green: "bg-emerald-400",
  yellow: "bg-yellow-400",
  red: "bg-red-400",
};

const CHECK_LABELS: Record<string, string> = {
  core_db: "Core Database",
  logs_db: "Logs Database",
  core_db_size: "Core DB Size",
  logs_db_size: "Logs DB Size",
  logs_retention: "Logs Retention",
  last_backup: "Last Backup",
};

export default function SystemHealthPanel() {
  const [health, setHealth] = useState<HealthData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchHealth = useCallback(async () => {
    try {
      const data: HealthData = await api.getSystemHealth();
      setHealth(data);
      setError(null);
    } catch (e) {
      // Never surface raw JSON/HTML bodies here (audit F16) — the client
      // humanizes ApiError messages; anything else gets a generic line.
      const msg = e instanceof Error ? e.message : "";
      setError(msg && !msg.trim().startsWith("{") && !msg.trim().startsWith("<")
        ? msg
        : "Health data unavailable — try again shortly.");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchHealth();
    const interval = setInterval(fetchHealth, 60_000);
    return () => clearInterval(interval);
  }, [fetchHealth]);

  if (loading) {
    return (
      <div className="border border-gray-700/50 rounded p-4">
        <h3 className="text-sm font-medium text-gray-400 mb-3">System Health</h3>
        <p className="text-xs text-gray-500">Loading...</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="border border-red-800/50 rounded p-4">
        <h3 className="text-sm font-medium text-gray-400 mb-3">System Health</h3>
        <p className="text-xs text-red-400 mb-2">{error}</p>
        <button
          onClick={() => { setLoading(true); fetchHealth(); }}
          className="text-xs text-cyan-400 hover:text-cyan-300 transition-colors"
        >
          Retry
        </button>
      </div>
    );
  }

  const overallStatus = health?.checks.some((c) => c.status === "red")
    ? "red"
    : health?.checks.some((c) => c.status === "yellow")
      ? "yellow"
      : "green";

  return (
    <div className="border border-gray-700/50 rounded p-4">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-gray-400">System Health</h3>
        <div className="flex items-center gap-1.5">
          <span
            className={`inline-block w-2 h-2 rounded-full ${STATUS_DOT[overallStatus]}`}
          />
          <span className={`text-xs ${STATUS_COLORS[overallStatus]}`}>
            {overallStatus === "green"
              ? "All systems operational"
              : overallStatus === "yellow"
                ? "Degraded"
                : "Issues detected"}
          </span>
        </div>
      </div>

      <div className="flex gap-4 mb-3 text-xs">
        <span className={health?.live.core_db ? "text-emerald-400" : "text-red-400"}>
          Core DB: {health?.live.core_db ? "connected" : "down"}
        </span>
        <span className={health?.live.logs_db ? "text-emerald-400" : "text-red-400"}>
          Logs DB: {health?.live.logs_db ? "connected" : "down"}
        </span>
      </div>

      <div className="space-y-1.5">
        {health?.checks.map((check) => (
          <div
            key={check.check_name}
            className="flex items-center justify-between text-xs"
          >
            <div className="flex items-center gap-1.5">
              <span
                className={`inline-block w-1.5 h-1.5 rounded-full ${STATUS_DOT[check.status] ?? "bg-gray-500"}`}
              />
              <span className="text-gray-300">
                {CHECK_LABELS[check.check_name] ?? check.check_name}
              </span>
            </div>
            <div className="flex items-center gap-2">
              {check.value && (
                <span className="text-gray-400 font-mono">{check.value}</span>
              )}
              {check.message && (
                <span className="text-gray-500 truncate max-w-48" title={check.message}>
                  {check.message}
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      {health?.checks[0] && (
        <p className="text-[10px] text-gray-600 mt-2">
          Last checked:{" "}
          {new Date(health.checks[0].checked_at).toLocaleString()}
        </p>
      )}
    </div>
  );
}
