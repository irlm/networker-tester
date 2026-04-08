import { useState, useEffect, useCallback } from "react";

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
      const token = localStorage.getItem("token");
      const res = await fetch("/api/system/health", {
        headers: { Authorization: `Bearer ${token}` },
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const data: HealthData = await res.json();
      setHealth(data);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to load health data");
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
      <div className="border border-zinc-700/50 rounded-lg p-4">
        <h3 className="text-sm font-medium text-zinc-400 mb-3">System Health</h3>
        <p className="text-xs text-zinc-500">Loading...</p>
      </div>
    );
  }

  if (error) {
    return (
      <div className="border border-red-800/50 rounded-lg p-4">
        <h3 className="text-sm font-medium text-zinc-400 mb-3">System Health</h3>
        <p className="text-xs text-red-400">{error}</p>
      </div>
    );
  }

  const overallStatus = health?.checks.some((c) => c.status === "red")
    ? "red"
    : health?.checks.some((c) => c.status === "yellow")
      ? "yellow"
      : "green";

  return (
    <div className="border border-zinc-700/50 rounded-lg p-4">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-zinc-400">System Health</h3>
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
                className={`inline-block w-1.5 h-1.5 rounded-full ${STATUS_DOT[check.status] ?? "bg-zinc-500"}`}
              />
              <span className="text-zinc-300">
                {CHECK_LABELS[check.check_name] ?? check.check_name}
              </span>
            </div>
            <div className="flex items-center gap-2">
              {check.value && (
                <span className="text-zinc-400 font-mono">{check.value}</span>
              )}
              {check.message && (
                <span className="text-zinc-500 truncate max-w-48" title={check.message}>
                  {check.message}
                </span>
              )}
            </div>
          </div>
        ))}
      </div>

      {health?.checks[0] && (
        <p className="text-[10px] text-zinc-600 mt-2">
          Last checked:{" "}
          {new Date(health.checks[0].checked_at).toLocaleString()}
        </p>
      )}
    </div>
  );
}
